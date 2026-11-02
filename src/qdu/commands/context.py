"""Shared command-layer context and configuration helpers."""

from __future__ import annotations

import sys
from argparse import Namespace
from pathlib import Path

from qdu.config import (
    ConfigRepository,
    apply_profile_overrides,
    given_profile_overrides,
)
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

    ``--exclude`` and ``--exclude-from`` add to the profile's patterns.

    Raises:
        UsageError: If an exclusion file cannot be read or the settings conflict.
    """
    profile = config.load_profile(args.profile)
    overrides = given_profile_overrides(args)
    added = list(getattr(args, "exclude", None) or [])
    for file_name in getattr(args, "exclude_from", None) or []:
        path = Path(file_name).expanduser()
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except OSError as exc:
            raise UsageError(f"cannot read exclusion file {path}: {exc}") from exc
        added.extend(
            line.strip()
            for line in lines
            if line.strip() and not line.strip().startswith("#")
        )
    if added:
        overrides["excludes"] = tuple(dict.fromkeys((*profile.excludes, *added)))
    return apply_profile_overrides(profile, overrides)
