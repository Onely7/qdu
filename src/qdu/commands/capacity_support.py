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


def _capacity_policy(args: Namespace, config: ConfigRepository) -> CapacityPolicy | None:
    profile = config.load_profile(args.profile)
    has_limit_override = hasattr(args, "capacity_limit_bytes")
    limit_bytes = (
        getattr(args, "capacity_limit_bytes")
        if has_limit_override
        else profile.capacity_limit_bytes
    )
    if has_limit_override and limit_bytes is None:
        return None
    user = (
        getattr(args, "capacity_user")
        if hasattr(args, "capacity_user")
        else profile.capacity_user
    )
    if limit_bytes is None:
        if user is not None:
            raise UsageError("--capacity-user requires --capacity-limit or a configured limit")
        return None
    return CapacityPolicy(limit_bytes=limit_bytes, user=user)


def _capacity_assessment(
    connection: sqlite3.Connection,
    metadata: dict[str, str],
    policy: CapacityPolicy | None,
) -> CapacityAssessment | None:
    if policy is None:
        return None
    if policy.user is None:
        return assess_capacity(
            target="profile root",
            limit_bytes=policy.limit_bytes,
            used_bytes=_directory_value(connection, "."),
        )
    if metadata.get("collect_users") != "true":
        raise UsageError(
            "capacity usage for one user requires owner statistics; "
            "take a new snapshot with '--with-users'"
        )
    uid = resolve_user(policy.user)
    row = connection.execute(
        "SELECT allocated_bytes FROM owners WHERE uid = ?", (uid,)
    ).fetchone()
    used_bytes = int(row[0]) if row is not None else 0
    username = username_for_uid(uid)
    return assess_capacity(
        target=f"user {username} (uid {uid})",
        limit_bytes=policy.limit_bytes,
        used_bytes=used_bytes,
    )


def _directory_value(connection: sqlite3.Connection, path: str) -> int:
    row = connection.execute(
        "SELECT allocated_bytes FROM directories WHERE path = ?", (path,)
    ).fetchone()
    return int(row[0]) if row is not None else 0


def _warn_capacity(
    renderer: Renderer, assessment: CapacityAssessment | None
) -> None:
    if assessment is None or not assessment.exceeded:
        return
    renderer.warning(
        "operational capacity limit exceeded by "
        f"{format_bytes(assessment.excess_bytes)}: "
        f"{format_bytes(assessment.used_bytes)} used of "
        f"{format_bytes(assessment.limit_bytes)} "
        f"({assessment.usage_percent:.1f}%; {assessment.target})"
    )


def _render_capacity_status(
    renderer: Renderer, assessment: CapacityAssessment
) -> None:
    renderer.heading("Operational capacity limit")
    status = (
        TableCell("EXCEEDED", "red")
        if assessment.exceeded
        else TableCell("within limit", "green")
    )
    remaining_label = "Exceeded by" if assessment.exceeded else "Remaining"
    remaining_value = (
        assessment.excess_bytes if assessment.exceeded else assessment.remaining_bytes
    )
    renderer.table(
        ["Target", "Used", "Allowed", "Usage", remaining_label, "Status"],
        [
            (
                assessment.target,
                format_bytes(assessment.used_bytes),
                format_bytes(assessment.limit_bytes),
                f"{assessment.usage_percent:.1f}%",
                format_bytes(remaining_value),
                status,
            )
        ],
        alignments=["left", "right", "right", "right", "right", "center"],
    )

