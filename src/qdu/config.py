"""Load and atomically persist qdu profile configuration."""

from __future__ import annotations

import configparser
import os
from dataclasses import replace
from pathlib import Path

from qdu.errors import UsageError
from qdu.models import ProfileConfig
from qdu.profile_names import validate_profile_name
from qdu.units import parse_size


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
        parser = self._read()
        defaults = parser["defaults"] if parser.has_section("defaults") else {}
        section_name = f"profile:{name}"
        section = parser[section_name] if parser.has_section(section_name) else {}

        def get(key: str, fallback: str) -> str:
            return str(section.get(key, defaults.get(key, fallback)))

        try:
            root_default = str(Path.home())
            excludes = tuple(
                line.strip() for line in get("exclude", "").splitlines() if line.strip()
            )
            record_max_depth_raw = get("record_max_depth", "").strip()
            capacity_limit_raw = get("capacity_limit", "").strip()
            capacity_user_raw = get("capacity_user", "").strip()
            return ProfileConfig(
                name=name,
                root=Path(get("path", root_default)).expanduser(),
                excludes=excludes,
                keep_snapshots=int(get("keep_snapshots", "100")),
                collect_users=get("collect_users", "false").lower()
                in {"1", "true", "yes", "on"},
                user_max_depth=int(get("user_max_depth", "3")),
                large_file_limit=int(get("large_file_limit", "1000")),
                record_max_depth=(
                    int(record_max_depth_raw) if record_max_depth_raw else None
                ),
                cross_filesystems=get("cross_filesystems", "false").lower()
                in {"1", "true", "yes", "on"},
                capacity_limit_bytes=(
                    parse_size(capacity_limit_raw) if capacity_limit_raw else None
                ),
                capacity_user=capacity_user_raw or None,
            )
        except (ValueError, TypeError) as exc:
            raise UsageError(
                f"invalid configuration for profile {name!r}: {exc}"
            ) from exc

    def save_profile(self, profile: ProfileConfig) -> None:
        """Atomically persist one validated profile with owner-only permissions."""
        self.validate_profile_name(profile.name)
        parser = self._read()
        section_name = f"profile:{profile.name}"
        if not parser.has_section(section_name):
            parser.add_section(section_name)
        section = parser[section_name]
        section["path"] = str(profile.root)
        section["exclude"] = "\n".join(profile.excludes)
        section["keep_snapshots"] = str(profile.keep_snapshots)
        section["collect_users"] = "true" if profile.collect_users else "false"
        section["user_max_depth"] = str(profile.user_max_depth)
        section["large_file_limit"] = str(profile.large_file_limit)
        section["record_max_depth"] = (
            "" if profile.record_max_depth is None else str(profile.record_max_depth)
        )
        section["cross_filesystems"] = "true" if profile.cross_filesystems else "false"
        section["capacity_limit"] = (
            ""
            if profile.capacity_limit_bytes is None
            else f"{profile.capacity_limit_bytes}B"
        )
        section["capacity_user"] = profile.capacity_user or ""
        self._write(parser)

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
            "keep_snapshots": "100",
            "collect_users": "false",
            "user_max_depth": "3",
            "large_file_limit": "1000",
            "record_max_depth": "",
            "cross_filesystems": "false",
            "capacity_limit": "",
            "capacity_user": "",
            "exclude": ".git\nnode_modules",
        }
        self._write(parser)
        return True


def merge_profile_overrides(
    profile: ProfileConfig,
    *,
    root: Path | None = None,
    excludes: tuple[str, ...] | None = None,
    keep_snapshots: int | None = None,
    collect_users: bool | None = None,
    user_max_depth: int | None = None,
    large_file_limit: int | None = None,
    record_max_depth: int | None | object = ...,
    cross_filesystems: bool | None = None,
    capacity_limit_bytes: int | None | object = ...,
    capacity_user: str | None | object = ...,
) -> ProfileConfig:
    """Return a validated profile with explicit command-line overrides applied.

    Ellipsis values preserve settings whose command-line option was omitted.

    Raises:
        UsageError: If the combined settings violate profile invariants.
    """
    values: dict[str, object] = {}
    if root is not None:
        values["root"] = root
    if excludes is not None:
        values["excludes"] = excludes
    if keep_snapshots is not None:
        values["keep_snapshots"] = keep_snapshots
    if collect_users is not None:
        values["collect_users"] = collect_users
    if user_max_depth is not None:
        values["user_max_depth"] = user_max_depth
    if large_file_limit is not None:
        values["large_file_limit"] = large_file_limit
    if record_max_depth is not ...:
        values["record_max_depth"] = record_max_depth
    if cross_filesystems is not None:
        values["cross_filesystems"] = cross_filesystems
    if capacity_limit_bytes is not ...:
        values["capacity_limit_bytes"] = capacity_limit_bytes
        if capacity_limit_bytes is None and capacity_user is ...:
            values["capacity_user"] = None
    if capacity_user is not ...:
        values["capacity_user"] = capacity_user
    try:
        return replace(profile, **values)
    except ValueError as exc:
        raise UsageError(f"invalid profile settings: {exc}") from exc
