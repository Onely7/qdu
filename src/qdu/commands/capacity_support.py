"""Internal helpers for evaluating and presenting capacity policies."""

from __future__ import annotations

import sqlite3
from argparse import Namespace

from qdu.capacity import CapacityAssessment, CapacityPolicy, assess_capacity
from qdu.config import ConfigRepository
from qdu.errors import (
    UsageError,
)
from qdu.query import (
    resolve_user,
)
from qdu.render import Renderer, TableCell
from qdu.scanner import username_for_uid
from qdu.units import format_bytes


def _capacity_policy(
    args: Namespace, config: ConfigRepository
) -> CapacityPolicy | None:
    profile = config.load_profile(args.profile)
    has_limit_override = hasattr(args, "capacity_limit_bytes")
    limit_bytes = (
        args.capacity_limit_bytes
        if has_limit_override
        else profile.capacity_limit_bytes
    )
    if has_limit_override and limit_bytes is None:
        return None
    user = (
        args.capacity_user if hasattr(args, "capacity_user") else profile.capacity_user
    )
    if limit_bytes is None:
        if user is not None:
            raise UsageError(
                "--capacity-user requires --capacity-limit or a configured limit"
            )
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


def _warn_capacity(renderer: Renderer, assessment: CapacityAssessment | None) -> None:
    if assessment is None or not assessment.exceeded:
        return
    renderer.warning(
        "operational capacity limit exceeded by "
        f"{format_bytes(assessment.excess_bytes)}: "
        f"{format_bytes(assessment.used_bytes)} used of "
        f"{format_bytes(assessment.limit_bytes)} "
        f"({assessment.usage_percent:.1f}%; {assessment.target})"
    )


def _render_capacity_status(renderer: Renderer, assessment: CapacityAssessment) -> None:
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
