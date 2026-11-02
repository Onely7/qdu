"""Commands for managing profiles and resolved configuration."""

from __future__ import annotations

import shutil
import time
from argparse import Namespace

from qdu.commands.context import build_renderer
from qdu.config import (
    PROFILE_SETTINGS,
    ConfigRepository,
    given_profile_overrides,
    render_profile_settings,
)
from qdu.errors import SnapshotFormatError, UsageError
from qdu.models import ProfileConfig
from qdu.render import Renderer
from qdu.storage import IndexRepository, ProfilePaths
from qdu.units import format_age, format_duration, format_exact_size, format_local_time

# How the table view describes a setting left empty; "not set" otherwise.
_UNSET_TEXT = {"record_max_depth": "unlimited", "exclude": "none"}


def profile_command(args: Namespace, config: ConfigRepository) -> int:
    """List, show, add, or remove a profile."""
    renderer = build_renderer(args)
    if args.profile_action == "list":
        return _list_profiles(renderer, config)
    if args.profile_action == "show":
        _show_profile(renderer, args.format, config.load_profile(args.name))
        return 0
    if args.profile_action == "add":
        overrides = given_profile_overrides(args)
        if overrides.get("capacity_limit_bytes", 0) is None:
            # Removing the limit also removes the user it applied to.
            overrides.setdefault("capacity_user", None)
        config.update_profile(args.name, render_profile_settings(overrides))
        renderer.message(f"Profile saved: {args.name}")
        return 0
    if args.profile_action == "remove":
        removed = config.remove_profile(args.name)
        if args.delete_data:
            paths = ProfilePaths.for_profile(args.name)
            if paths.root.exists():
                shutil.rmtree(paths.root)
        renderer.message(
            f"Profile removed: {args.name}"
            if removed
            else f"Profile was not configured: {args.name}"
        )
        return 0
    raise UsageError("unknown profile action")


def config_command(args: Namespace, config: ConfigRepository) -> int:
    """Render the effective configuration for a profile."""
    renderer = build_renderer(args)
    if args.config_action == "path":
        renderer.message(str(config.path))
        return 0
    if args.config_action == "init":
        created = config.initialize()
        renderer.message(
            f"Created {config.path}" if created else f"Already exists: {config.path}"
        )
        return 0
    if args.config_action == "show":
        _show_profile(renderer, args.format, config.load_profile(args.profile))
        return 0
    raise UsageError("unknown config action")


def _list_profiles(renderer: Renderer, config: ConfigRepository) -> int:
    rows: list[tuple[str, str, str, str, str]] = []
    problems: list[str] = []
    now = int(time.time())
    for name in config.list_profiles():
        try:
            count, latest = _snapshot_overview(name, now)
        except SnapshotFormatError as exc:
            problems.append(f"{exc}; run 'qdu repair --profile {name}'")
            count, latest = "?", "(invalid index)"
        try:
            profile = config.load_profile(name)
        except UsageError as exc:
            # Keep listing the other profiles; one broken section should not
            # hide them.
            problems.append(str(exc))
            rows.append((name, count, latest, "", "(invalid configuration)"))
            continue
        rows.append(
            (name, count, latest, _profile_capacity_text(profile), str(profile.root))
        )
    renderer.table(
        ["Profile", "Snapshots", "Latest", "Capacity", "Root"],
        rows,
        alignments=["left", "right", "left", "left", "left"],
    )
    for message in problems:
        renderer.warning(message)
    return 2 if problems else 0


def _snapshot_overview(name: str, now: int) -> tuple[str, str]:
    """Return a profile's snapshot count and when its newest snapshot was taken.

    Raises:
        SnapshotFormatError: If the profile's index cannot be read.
    """
    snapshots = IndexRepository(ProfilePaths.for_profile(name)).load().snapshots
    if not snapshots:
        return "0", "none"
    newest = snapshots[-1]
    latest = (
        f"{format_local_time(newest.created_epoch)} "
        f"({format_age(now - newest.created_epoch)} ago)"
    )
    if not newest.complete:
        latest += ", incomplete"
    return f"{len(snapshots):,}", latest


def _show_profile(
    renderer: Renderer, output_format: str, profile: ProfileConfig
) -> None:
    if output_format == "json":
        renderer.json(_profile_json(profile))
        return
    settings = _profile_settings(profile)
    if output_format == "tsv":
        # One row per exclusion pattern; an unset value is empty.
        renderer.tsv(
            ["key", "value"],
            [
                (key, item)
                for key, value in settings
                for item in (value if isinstance(value, tuple) else (value,))
            ],
        )
        return
    renderer.key_values(
        [(key, value or _UNSET_TEXT.get(key, "not set")) for key, value in settings]
    )


def _profile_settings(
    profile: ProfileConfig,
) -> list[tuple[str, str | tuple[str, ...]]]:
    """Return each setting as configuration text; exclusions stay one per item."""
    settings: list[tuple[str, str | tuple[str, ...]]] = [("profile", profile.name)]
    for setting in PROFILE_SETTINGS:
        value = getattr(profile, setting.field)
        settings.append(
            (setting.key, value if setting.key == "exclude" else setting.render(value))
        )
    return settings


def _profile_capacity_text(profile: ProfileConfig) -> str:
    if profile.capacity_limit_bytes is None:
        return "not set"
    target = profile.capacity_user or "profile root"
    return f"{format_exact_size(profile.capacity_limit_bytes)} ({target})"


def _profile_json(profile: ProfileConfig) -> dict[str, object]:
    return {
        "name": profile.name,
        "path": str(profile.root),
        "exclude": list(profile.excludes),
        "keep_snapshots": profile.keep_snapshots,
        "compact_after": (
            "off"
            if profile.compact_after_seconds is None
            else format_duration(profile.compact_after_seconds)
        ),
        "collect_users": profile.collect_users,
        "user_max_depth": profile.user_max_depth,
        "large_file_limit": profile.large_file_limit,
        "record_max_depth": profile.record_max_depth,
        "cross_filesystems": profile.cross_filesystems,
        "capacity_limit": (
            format_exact_size(profile.capacity_limit_bytes)
            if profile.capacity_limit_bytes is not None
            else None
        ),
        "capacity_limit_bytes": profile.capacity_limit_bytes,
        "capacity_user": profile.capacity_user,
    }
