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


def build_renderer(args: Namespace) -> Renderer:
    return Renderer(
        RenderOptions(
            output_format=getattr(args, "format", "table"),
            style=getattr(args, "style", "auto"),
            color=getattr(args, "color", "auto"),
            stream=sys.stdout,
        )
    )


def load_profile(args: Namespace, config: ConfigRepository) -> object:
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
            getattr(args, "record_max_depth")
            if hasattr(args, "record_max_depth")
            else ...
        ),
        cross_filesystems=getattr(args, "cross_filesystems", None),
        capacity_limit_bytes=(
            getattr(args, "capacity_limit_bytes")
            if hasattr(args, "capacity_limit_bytes")
            else ...
        ),
        capacity_user=(
            getattr(args, "capacity_user") if hasattr(args, "capacity_user") else ...
        ),
    )


