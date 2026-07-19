"""Shared command-layer context and configuration helpers."""

from __future__ import annotations

import sys
from argparse import Namespace
from pathlib import Path

from qdu.config import ConfigRepository, merge_profile_overrides
from qdu.errors import (
    UsageError,
)
from qdu.models import ProfileConfig
from qdu.render import Renderer, RenderOptions


def build_renderer(args: Namespace) -> Renderer:
    """Build a renderer from common CLI output options."""
    return Renderer(
        RenderOptions(
            output_format=getattr(args, "format", "table"),
            style=getattr(args, "style", "auto"),
            color=getattr(args, "color", "auto"),
            stream=sys.stdout,
        )
    )


def load_profile(args: Namespace, config: ConfigRepository) -> ProfileConfig:
    """Load a profile and apply command-line scan overrides.

    Raises:
        UsageError: If an exclusion file cannot be read.
    """
    profile = config.load_profile(args.profile)
    excludes = list(profile.excludes)
    for pattern in getattr(args, "exclude", []) or []:
        excludes.append(pattern)
    for file_name in getattr(args, "exclude_from", []) or []:
        path = Path(file_name).expanduser()
        try:
            for line in path.read_text(encoding="utf-8").splitlines():
                stripped = line.strip()
                if stripped and not stripped.startswith("#"):
                    excludes.append(stripped)
        except OSError as exc:
            raise UsageError(f"cannot read exclusion file {path}: {exc}") from exc
    root = Path(args.path).expanduser() if getattr(args, "path", None) else None
    collect_users = getattr(args, "with_users", None)
    return merge_profile_overrides(
        profile,
        root=root,
        excludes=tuple(dict.fromkeys(excludes)),
        keep_snapshots=getattr(args, "keep_snapshots", None),
        collect_users=collect_users,
        user_max_depth=getattr(args, "user_max_depth", None),
        large_file_limit=getattr(args, "file_top", None),
        record_max_depth=(
            args.record_max_depth if hasattr(args, "record_max_depth") else ...
        ),
        cross_filesystems=getattr(args, "cross_filesystems", None),
        capacity_limit_bytes=(
            args.capacity_limit_bytes if hasattr(args, "capacity_limit_bytes") else ...
        ),
        capacity_user=(args.capacity_user if hasattr(args, "capacity_user") else ...),
    )
