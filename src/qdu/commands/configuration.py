from __future__ import annotations

import dataclasses
import gzip
import json
import os
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
from argparse import Namespace
from contextlib import ExitStack
from datetime import datetime
from pathlib import Path
from typing import Sequence

from qdu.capacity import CapacityAssessment, CapacityPolicy, assess_capacity
from qdu.config import ConfigRepository, merge_profile_overrides
from qdu.errors import BusyError, SnapshotFormatError, ThresholdExceeded, UsageError, VerificationError
from qdu.live import largest_files_live
from qdu.locking import ProfileLock, clear_stale_lock, inspect_lock
from qdu.models import (
    CheckResult,
    DirectoryRecord,
    DoctorItem,
    FileRecord,
    ProfileIndex,
    SnapshotIndexRecord,
)
from qdu.patterns import PathPatternMatcher
from qdu.query import (
    SnapshotQueryService,
    normalize_relative_path,
    relative_to_scope,
    resolve_user,
)
from qdu.render import RenderOptions, Renderer, TableCell, bar, percent
from qdu.repository import SnapshotRepository
from qdu.staleness import (
    StaleLevel,
    StaleThresholds,
    assess_staleness,
    stale_cutoff_epoch,
)
from qdu.storage import (
    INDEX_VERSION,
    IndexRepository,
    ProfilePaths,
    gzip_snapshot,
    materialized_snapshot,
    sha256_file,
    validate_database,
)
from qdu.units import format_age, format_bytes, parse_duration, parse_size
from qdu.scanner import username_for_uid

from qdu.commands.context import build_renderer


def profile_command(args: Namespace, config: ConfigRepository) -> int:
    renderer = build_renderer(args)
    if args.profile_action == "list":
        names = config.list_profiles()
        renderer.table(
            ["Profile", "Root", "Capacity"],
            [
                (
                    name,
                    str(config.load_profile(name).root),
                    _profile_capacity_text(config.load_profile(name)),
                )
                for name in names
            ],
        )
        return 0
    if args.profile_action == "show":
        profile = config.load_profile(args.name)
        if args.format == "json":
            renderer.json(_profile_json(profile))
        else:
            renderer.key_values(list(_profile_json(profile).items()))
        return 0
    if args.profile_action == "add":
        existing = config.load_profile(args.name)
        profile = merge_profile_overrides(
            existing,
            root=Path(args.path).expanduser(),
            excludes=tuple(args.exclude or ()),
            keep_snapshots=args.keep_snapshots,
            collect_users=args.with_users,
            user_max_depth=args.user_max_depth,
            large_file_limit=args.file_top,
            record_max_depth=args.record_max_depth,
            cross_filesystems=args.cross_filesystems,
            capacity_limit_bytes=(
                getattr(args, "capacity_limit_bytes")
                if hasattr(args, "capacity_limit_bytes")
                else ...
            ),
            capacity_user=(
                getattr(args, "capacity_user")
                if hasattr(args, "capacity_user")
                else ...
            ),
        )
        if profile.capacity_limit_bytes is None and profile.capacity_user is not None:
            profile = dataclasses.replace(profile, capacity_user=None)
        config.save_profile(profile)
        renderer.message(f"Profile saved: {args.name}")
        return 0
    if args.profile_action == "remove":
        removed = config.remove_profile(args.name)
        if args.delete_data:
            paths = ProfilePaths.for_profile(args.name)
            if paths.root.exists():
                shutil.rmtree(paths.root)
        renderer.message(f"Profile removed: {args.name}" if removed else f"Profile was not configured: {args.name}")
        return 0
    raise UsageError("unknown profile action")


def config_command(args: Namespace, config: ConfigRepository) -> int:
    renderer = build_renderer(args)
    if args.config_action == "path":
        renderer.message(str(config.path))
        return 0
    if args.config_action == "init":
        created = config.initialize()
        renderer.message(f"Created {config.path}" if created else f"Already exists: {config.path}")
        return 0
    if args.config_action == "show":
        profile = config.load_profile(args.profile)
        if args.format == "json":
            renderer.json(_profile_json(profile))
        else:
            renderer.key_values(list(_profile_json(profile).items()))
        return 0
    raise UsageError("unknown config action")


def _profile_capacity_text(profile: object) -> str:
    if profile.capacity_limit_bytes is None:
        return "not set"
    target = profile.capacity_user or "profile root"
    return f"{format_bytes(profile.capacity_limit_bytes)} ({target})"

def _profile_json(profile: object) -> dict[str, object]:
    return {
        "name": profile.name,
        "path": str(profile.root),
        "exclude": list(profile.excludes),
        "keep_snapshots": profile.keep_snapshots,
        "collect_users": profile.collect_users,
        "user_max_depth": profile.user_max_depth,
        "large_file_limit": profile.large_file_limit,
        "record_max_depth": profile.record_max_depth,
        "cross_filesystems": profile.cross_filesystems,
        "capacity_limit": (
            format_bytes(profile.capacity_limit_bytes)
            if profile.capacity_limit_bytes is not None
            else None
        ),
        "capacity_limit_bytes": profile.capacity_limit_bytes,
        "capacity_user": profile.capacity_user,
    }
