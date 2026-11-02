"""Parse and apply gitignore-style exclusion patterns to root-relative paths."""

from __future__ import annotations

import hashlib
import json
import re
from collections.abc import Iterable
from dataclasses import dataclass

from qdu.errors import UsageError


@dataclass(frozen=True, slots=True)
class _Rule:
    regex: re.Pattern[str]
    anchored: bool
    directory_only: bool


class PathPatternMatcher:
    """Apply one documented exclusion contract across all scanning features.

    Patterns follow the gitignore conventions without negation:

    - A pattern without a slash, such as ``.git`` or ``*.tmp``, matches a file
      or directory name at any depth.
    - A pattern with a slash at the beginning or in the middle, such as
      ``/build`` or ``data/raw``, matches the path from the scan root.
    - A trailing slash, as in ``cache/``, matches directories only.
    - ``*`` and ``?`` do not cross a slash; ``[...]`` is a character class.
      ``**/`` matches any number of leading directories, ``/**`` everything
      inside a directory, and ``/**/`` zero or more directories.
    - A backslash makes the next character literal, and matching is
      case-sensitive.

    An excluded directory is not entered, so nothing below it is counted.
    """

    def __init__(self, patterns: tuple[str, ...]) -> None:
        self._patterns = tuple(
            pattern.strip() for pattern in patterns if pattern.strip()
        )
        rules = [_compile(pattern) for pattern in self._patterns]
        # Rules for entries of any type, then rules for directories only; each
        # pair splits name rules from rules anchored to the scan root.
        self._entry_name = _combine(
            rule for rule in rules if not rule.directory_only and not rule.anchored
        )
        self._entry_path = _combine(
            rule for rule in rules if not rule.directory_only and rule.anchored
        )
        self._directory_name = _combine(
            rule for rule in rules if rule.directory_only and not rule.anchored
        )
        self._directory_path = _combine(
            rule for rule in rules if rule.directory_only and rule.anchored
        )

    @property
    def patterns(self) -> tuple[str, ...]:
        """Patterns as given, without surrounding whitespace or blank entries."""
        return self._patterns

    @property
    def has_directory_rules(self) -> bool:
        """Whether some pattern can only be decided once an entry is a directory."""
        return self._directory_name is not None or self._directory_path is not None

    @property
    def digest(self) -> str:
        """Stable SHA-256 digest used to compare snapshot scan policies."""
        payload = json.dumps(
            {"syntax": "gitignore", "patterns": self._patterns}
            if self._patterns
            else [],
            ensure_ascii=False,
            separators=(",", ":"),
        )
        return hashlib.sha256(payload.encode("utf-8")).hexdigest()

    def matches(self, relative_path: str, *, is_directory: bool = False) -> bool:
        """Return whether a path, or any directory above it, is excluded."""
        parts = [part for part in relative_path.split("/") if part not in {"", "."}]
        for index, name in enumerate(parts):
            prefix = "/".join(parts[: index + 1])
            last = index == len(parts) - 1
            if self.matches_entry(name, prefix) or (
                (is_directory or not last) and self.matches_directory(name, prefix)
            ):
                return True
        return False

    def matches_entry(self, name: str, relative_path: str) -> bool:
        """Return whether a walker should skip an entry of any type.

        Directory-only patterns are left to :meth:`matches_directory`. A walker
        never enters an excluded directory, so the entry's ancestors have
        already passed.

        Args:
            name: The entry's own name, the last component of ``relative_path``.
            relative_path: The entry's root-relative path.
        """
        return bool(
            (self._entry_name is not None and self._entry_name.fullmatch(name))
            or (
                self._entry_path is not None
                and self._entry_path.fullmatch(relative_path)
            )
        )

    def matches_directory(self, name: str, relative_path: str) -> bool:
        """Return whether a directory entry matches a directory-only pattern."""
        return bool(
            (self._directory_name is not None and self._directory_name.fullmatch(name))
            or (
                self._directory_path is not None
                and self._directory_path.fullmatch(relative_path)
            )
        )


def _compile(pattern: str) -> _Rule:
    if pattern.startswith("!"):
        raise UsageError(
            f"negated exclusion patterns are not supported: {pattern} "
            "(write \\! for a name that starts with '!')"
        )
    directory_only = pattern.endswith("/") and not pattern.endswith("\\/")
    body = pattern.rstrip("/") if directory_only else pattern
    if body.startswith("./"):
        body = "/" + body[2:]
    anchored = "/" in body
    body = body.lstrip("/")
    if body in {"", "."}:
        raise UsageError(f"exclusion pattern matches nothing: {pattern}")
    return _Rule(
        regex=re.compile(_translate(body)),
        anchored=anchored,
        directory_only=directory_only,
    )


def _combine(rules: Iterable[_Rule]) -> re.Pattern[str] | None:
    expressions = [f"(?:{rule.regex.pattern})" for rule in rules]
    return re.compile("|".join(expressions)) if expressions else None


def _translate(glob: str) -> str:
    """Translate a slash-separated glob into a regular expression for fullmatch."""
    output: list[str] = []
    index = 0
    length = len(glob)
    while index < length:
        character = glob[index]
        if character == "*":
            end = index
            while end < length and glob[end] == "*":
                end += 1
            whole_segment = (index == 0 or glob[index - 1] == "/") and (
                end == length or glob[end] == "/"
            )
            if end - index >= 2 and whole_segment:
                if end == length:
                    output.append(".*")  # "dir/**": everything inside dir
                else:
                    output.append("(?:.*/)?")  # "**/": zero or more directories
                    end += 1
            else:
                output.append("[^/]*")
            index = end
        elif character == "?":
            output.append("[^/]")
            index += 1
        elif character == "[":
            close = _class_end(glob, index)
            if close is None:
                output.append(re.escape(character))
                index += 1
                continue
            body = glob[index + 1 : close]
            negated = body[:1] in {"!", "^"}
            members = "".join(
                item if item == "-" else re.escape(item)
                for item in (body[1:] if negated else body)
            )
            output.append(f"(?!/)[{'^' if negated else ''}{members}]")
            index = close + 1
        elif character == "\\" and index + 1 < length:
            output.append(re.escape(glob[index + 1]))
            index += 2
        else:
            output.append(re.escape(character))
            index += 1
    return "".join(output)


def _class_end(glob: str, start: int) -> int | None:
    index = start + 1
    if index < len(glob) and glob[index] in {"!", "^"}:
        index += 1
    if index < len(glob) and glob[index] == "]":
        index += 1
    while index < len(glob) and glob[index] != "]":
        index += 1
    return index if index < len(glob) else None
