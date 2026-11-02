"""Load and atomically persist qdu profile configuration."""

from __future__ import annotations

import configparser
import os
from collections.abc import Callable, Mapping
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from qdu.errors import UsageError
from qdu.models import ProfileConfig
from qdu.profile_names import validate_profile_name
from qdu.units import (
    format_duration,
    format_exact_size,
    parse_duration,
    parse_size,
)


def xdg_config_home() -> Path:
    """Return the effective XDG configuration directory without creating it."""
    raw = os.environ.get("XDG_CONFIG_HOME")
    if raw:
        return Path(raw).expanduser()
    return Path.home() / ".config"


def xdg_state_home() -> Path:
    """Return the effective XDG state directory without creating it."""
    raw = os.environ.get("XDG_STATE_HOME")
    if raw:
        return Path(raw).expanduser()
    return Path.home() / ".local" / "state"


@dataclass(frozen=True, slots=True)
class ProfileSetting:
    """One profile setting: its configuration key, ProfileConfig field, and text.

    ``default`` applies when neither the profile section nor ``[defaults]``
    sets the key. Command-line options for a setting store into ``field`` and
    default to ``argparse.SUPPRESS``, so only given options appear on the
    parsed namespace.
    """

    key: str
    field: str
    default: str
    parse: Callable[[str], Any]
    render: Callable[[Any], str]


def _parse_path(raw: str) -> Path:
    return Path(raw.strip()).expanduser()


def _parse_lines(raw: str) -> tuple[str, ...]:
    return tuple(line.strip() for line in raw.splitlines() if line.strip())


def _parse_bool(raw: str) -> bool:
    return raw.strip().lower() in {"1", "true", "yes", "on"}


def _render_bool(value: bool) -> str:
    return "true" if value else "false"


def _parse_optional_int(raw: str) -> int | None:
    return int(raw) if raw.strip() else None


def _parse_optional_size(raw: str) -> int | None:
    return parse_size(raw) if raw.strip() else None


def _render_size(value: int | None) -> str:
    return "" if value is None else format_exact_size(value)


def _parse_optional_text(raw: str) -> str | None:
    return raw.strip() or None


def _render_optional(value: object | None) -> str:
    return "" if value is None else str(value)


def parse_compact_after(raw: str) -> int | None:
    """Parse a compaction age, where ``off`` or ``never`` disables compaction.

    Raises:
        UsageError: If the value is neither a duration nor ``off``/``never``.
    """
    value = raw.strip()
    if value.lower() in {"off", "never"}:
        return None
    return parse_duration(value)


def _render_compact_after(seconds: int | None) -> str:
    return "off" if seconds is None else format_duration(seconds)


PROFILE_SETTINGS: tuple[ProfileSetting, ...] = (
    ProfileSetting("path", "root", "~", _parse_path, str),
    ProfileSetting("exclude", "excludes", "", _parse_lines, "\n".join),
    # About a year of weekly snapshots.
    ProfileSetting("keep_snapshots", "keep_snapshots", "52", int, str),
    ProfileSetting(
        "compact_after",
        "compact_after_seconds",
        "7d",
        parse_compact_after,
        _render_compact_after,
    ),
    ProfileSetting(
        "collect_users", "collect_users", "false", _parse_bool, _render_bool
    ),
    ProfileSetting("user_max_depth", "user_max_depth", "3", int, str),
    ProfileSetting("large_file_limit", "large_file_limit", "1000", int, str),
    ProfileSetting(
        "record_max_depth",
        "record_max_depth",
        "",
        _parse_optional_int,
        _render_optional,
    ),
    ProfileSetting(
        "cross_filesystems", "cross_filesystems", "false", _parse_bool, _render_bool
    ),
    ProfileSetting(
        "capacity_limit",
        "capacity_limit_bytes",
        "",
        _parse_optional_size,
        _render_size,
    ),
    ProfileSetting(
        "capacity_user",
        "capacity_user",
        "",
        _parse_optional_text,
        _render_optional,
    ),
)

# `qdu config init` also excludes version-control and dependency folders.
_INITIAL_DEFAULTS = {"exclude": ".git\nnode_modules"}


class ConfigRepository:
    """Read and atomically write profile settings in one INI file."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path or xdg_config_home() / "qdu" / "config.ini"

    def _read(self) -> configparser.ConfigParser:
        parser = configparser.ConfigParser(interpolation=None)
        if self.path.exists():
            parser.read(self.path, encoding="utf-8")
        return parser

    def _write(self, parser: configparser.ConfigParser) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        temporary = self.path.with_suffix(".tmp")
        with temporary.open("w", encoding="utf-8") as handle:
            parser.write(handle)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, self.path)

    @staticmethod
    def validate_profile_name(name: str) -> None:
        """Reject names that cannot safely identify one profile path component."""
        validate_profile_name(name)

    def list_profiles(self) -> list[str]:
        """Return configured profile names plus the implicit default profile."""
        parser = self._read()
        names = [
            section.removeprefix("profile:")
            for section in parser.sections()
            if section.startswith("profile:")
        ]
        if "default" not in names:
            names.append("default")
        return sorted(set(names))

    def load_profile(self, name: str) -> ProfileConfig:
        """Resolve defaults and named overrides into validated profile settings.

        Raises:
            UsageError: If the name or any stored setting is invalid.
        """
        self.validate_profile_name(name)
        return _profile_from(self._read(), name)

    def update_profile(self, name: str, settings: Mapping[str, str]) -> ProfileConfig:
        """Store the given profile settings and keep every other stored value.

        A setting absent from both ``settings`` and the profile section keeps
        following the ``[defaults]`` section. Nothing is written when the
        resulting profile is invalid.

        Args:
            name: Profile to create or update.
            settings: Raw configuration values keyed by configuration key.

        Returns:
            The validated profile as it is now stored.

        Raises:
            UsageError: If the name or the resulting settings are invalid.
        """
        self.validate_profile_name(name)
        parser = self._read()
        section_name = f"profile:{name}"
        if not parser.has_section(section_name):
            parser.add_section(section_name)
        for key, value in settings.items():
            parser[section_name][key] = value
        profile = _profile_from(parser, name)
        self._write(parser)
        return profile

    def remove_profile(self, name: str) -> bool:
        """Remove a profile section without deleting its snapshot state."""
        self.validate_profile_name(name)
        parser = self._read()
        removed = parser.remove_section(f"profile:{name}")
        if removed:
            self._write(parser)
        return removed

    def initialize(self) -> bool:
        """Create the default configuration unless a file already exists."""
        if self.path.exists():
            return False
        parser = configparser.ConfigParser(interpolation=None)
        parser["defaults"] = {
            setting.key: _INITIAL_DEFAULTS.get(setting.key, setting.default)
            for setting in PROFILE_SETTINGS
            if setting.field != "root"
        }
        self._write(parser)
        return True


def _profile_from(parser: configparser.ConfigParser, name: str) -> ProfileConfig:
    defaults = parser["defaults"] if parser.has_section("defaults") else {}
    section_name = f"profile:{name}"
    section = parser[section_name] if parser.has_section(section_name) else {}
    try:
        values = {
            setting.field: setting.parse(
                str(
                    section.get(setting.key, defaults.get(setting.key, setting.default))
                )
            )
            for setting in PROFILE_SETTINGS
        }
        return ProfileConfig(name=name, **values)
    except (ValueError, TypeError, UsageError) as exc:
        raise UsageError(f"invalid configuration for profile {name!r}: {exc}") from exc


def given_profile_overrides(namespace: object) -> dict[str, Any]:
    """Return the ProfileConfig fields given as attributes of a parsed namespace."""
    return {
        setting.field: getattr(namespace, setting.field)
        for setting in PROFILE_SETTINGS
        if hasattr(namespace, setting.field)
    }


def render_profile_settings(overrides: Mapping[str, Any]) -> dict[str, str]:
    """Return configuration text for the given ProfileConfig field values."""
    return {
        setting.key: setting.render(overrides[setting.field])
        for setting in PROFILE_SETTINGS
        if setting.field in overrides
    }


def apply_profile_overrides(
    profile: ProfileConfig, overrides: Mapping[str, Any]
) -> ProfileConfig:
    """Return a validated profile with command-line field values applied.

    Raises:
        UsageError: If the combined settings violate profile invariants.
    """
    try:
        return replace(profile, **overrides)
    except ValueError as exc:
        raise UsageError(f"invalid profile settings: {exc}") from exc
