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
from qdu.commands.snapshots import (
    _filesystem_json,
    _record_json,
    _stale_enabled,
    _stale_style,
    diff_command,
    show_command,
)


def users_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    record = service.resolve(args.snapshot)
    with ExitStack() as stack:
        context = service.open_context(stack, record)
        if context.metadata.get("collect_users") != "true":
            raise UsageError("this snapshot has no owner statistics; use 'qdu snapshot --with-users'")
        _render_users(
            renderer,
            context.connection,
            service,
            top=args.top,
            directories_per_user=args.dirs,
            output_format=args.format,
            metric=args.metric,
            record=record,
        )
    return 0


def files_command(args: Namespace, config: ConfigRepository) -> int:
    renderer = build_renderer(args)
    stale_enabled = _stale_enabled(args)
    if args.live:
        profile = config.load_profile(args.profile)
        matcher = PathPatternMatcher(profile.excludes)
        if args.path:
            live_root = Path(args.path).expanduser()
        else:
            try:
                live_root = Path(SnapshotQueryService(args.profile).resolve(args.snapshot).root)
            except UsageError:
                live_root = profile.root
        reference_epoch = int(time.time())
        records, errors = largest_files_live(
            root=live_root,
            matcher=matcher,
            top=args.top,
            under=args.under,
            min_size=parse_size(args.min_size),
            user=args.user,
            modified_before_epoch=stale_cutoff_epoch(
                reference_epoch, args.stale_only
            ),
        )
        for message in errors[:10]:
            renderer.warning(message)
        source = "live scan"
        limit_note = None
    else:
        service = SnapshotQueryService(args.profile)
        record = service.resolve(args.snapshot)
        reference_epoch = record.created_epoch
        with ExitStack() as stack:
            context = service.open_context(stack, record)
            records = service.large_files(
                context.connection,
                top=args.top,
                under=args.under,
                min_size=parse_size(args.min_size),
                user=args.user,
                modified_before_epoch=stale_cutoff_epoch(
                    reference_epoch, args.stale_only
                ),
            )
            stored_limit = int(context.metadata.get("large_file_limit", "0"))
        source = record.snapshot_id
        limit_note = stored_limit
    _render_files(
        renderer,
        records,
        source=source,
        output_format=args.format,
        stored_limit=limit_note,
        stale_enabled=stale_enabled,
        reference_epoch=reference_epoch,
        thresholds=args.stale_thresholds,
    )
    return 0

def inodes_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    record = service.resolve(args.snapshot)
    with ExitStack() as stack:
        context = service.open_context(stack, record)
        if args.users:
            _render_users(
                renderer,
                context.connection,
                service,
                top=args.top,
                directories_per_user=args.dirs,
                output_format=args.format,
                metric="inode_count",
                record=record,
            )
        else:
            scope, rows = service.directory_ranking(
                context.connection,
                under=args.under,
                max_depth=args.max_depth,
                top=args.top,
                min_size=0,
                match=args.match,
                metric="inode_count",
            )
            assert scope is not None
            if args.format == "json":
                renderer.json(
                    {
                        "snapshot": _record_json(record),
                        "scope": scope.path,
                        "entries": [
                            {
                                "rank": rank,
                                "path": relative_to_scope(item.path, scope.path),
                                "inode_count": item.inode_count,
                                "file_count": item.file_count,
                                "directory_count": item.directory_count,
                            }
                            for rank, item in enumerate(rows, 1)
                        ],
                        "filesystem": _filesystem_json(context.metadata, Path(record.root)),
                    }
                )
            elif args.format == "tsv":
                renderer.tsv(
                    ["rank", "inode_count", "file_count", "directory_count", "path"],
                    [
                        (
                            rank,
                            item.inode_count,
                            item.file_count,
                            item.directory_count,
                            relative_to_scope(item.path, scope.path),
                        )
                        for rank, item in enumerate(rows, 1)
                    ],
                )
            else:
                filesystem = _filesystem_json(context.metadata, Path(record.root))
                renderer.heading("Inode usage")
                renderer.key_values(
                    [
                        ("Snapshot", record.snapshot_id),
                        ("Scope", scope.path),
                        ("Snapshot free inodes", f"{filesystem['snapshot']['available_inodes']:,}"),
                        ("Current free inodes", f"{filesystem['current']['available_inodes']:,}" if filesystem.get("current") else "unavailable"),
                    ]
                )
                renderer.message("")
                renderer.table(
                    ["Rank", "Inodes", "Files", "Dirs", "Path"],
                    [
                        (
                            rank,
                            f"{item.inode_count:,}",
                            f"{item.file_count:,}",
                            f"{item.directory_count:,}",
                            relative_to_scope(item.path, scope.path),
                        )
                        for rank, item in enumerate(rows, 1)
                    ],
                    alignments=["right", "right", "right", "right", "left"],
                )
    return 0


def explain_command(args: Namespace, config: ConfigRepository) -> int:
    if args.diff:
        args.under = args.path_argument
        args.max_depth = 1
        return diff_command(args)
    args.under = args.path_argument
    args.max_depth = 1
    return show_command(args, config)


def browse_command(args: Namespace, config: ConfigRepository) -> int:
    if shutil.which("fzf") is None:
        raise UsageError("fzf is not installed; use 'qdu explain PATH' instead")
    service = SnapshotQueryService(args.profile)
    record = service.resolve(args.snapshot)
    with ExitStack() as stack:
        context = service.open_context(stack, record)
        process = subprocess.Popen(
            ["fzf", "--prompt", "qdu> ", "--height", "80%", "--reverse"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        assert process.stdin is not None
        try:
            for row in context.connection.execute("SELECT path FROM directories ORDER BY path"):
                process.stdin.write(f"{row[0]}\n")
        except BrokenPipeError:
            pass
        finally:
            process.stdin.close()
        assert process.stdout is not None
        selected = process.stdout.read().strip()
        return_code = process.wait()
    if return_code != 0 or not selected:
        return 0
    args.under = selected
    return show_command(args, config)


def _render_users(
    renderer: Renderer,
    connection: sqlite3.Connection,
    service: SnapshotQueryService,
    *,
    top: int,
    directories_per_user: int,
    output_format: str,
    metric: str,
    record: SnapshotIndexRecord | None = None,
) -> None:
    owners, directories, metric_total = service.owners(
        connection,
        top=top,
        directories_per_user=directories_per_user,
        metric=metric,
    )
    total = metric_total
    if output_format == "json":
        renderer.json(
            {
                "snapshot": _record_json(record) if record else None,
                "metric": metric,
                "users": [
                    {
                        **dataclasses.asdict(owner),
                        "directories": [dataclasses.asdict(item) for item in directories.get(owner.uid, [])],
                    }
                    for owner in owners
                ],
            }
        )
        return
    if output_format == "tsv":
        rows = []
        for rank, owner in enumerate(owners, 1):
            rows.append(("user", rank, owner.uid, owner.username, owner.allocated_bytes, owner.inode_count, ""))
            for directory in directories.get(owner.uid, []):
                rows.append(("directory", rank, owner.uid, owner.username, directory.allocated_bytes, directory.inode_count, directory.path))
        renderer.tsv(["type", "rank", "uid", "user", "allocated_bytes", "inode_count", "path"], rows)
        return
    renderer.heading("User usage")
    if metric == "inode_count":
        renderer.table(
            ["Rank", "User", "UID", "Inodes", "Share", "Size"],
            [
                (
                    rank,
                    owner.username,
                    owner.uid,
                    f"{owner.inode_count:,}",
                    percent(owner.inode_count, total),
                    format_bytes(owner.allocated_bytes),
                )
                for rank, owner in enumerate(owners, 1)
            ],
            alignments=["right", "left", "right", "right", "right", "right"],
        )
    else:
        renderer.table(
            ["Rank", "User", "UID", "Size", "Share", "Inodes"],
            [
                (
                    rank,
                    owner.username,
                    owner.uid,
                    format_bytes(owner.allocated_bytes),
                    percent(owner.allocated_bytes, total),
                    f"{owner.inode_count:,}",
                )
                for rank, owner in enumerate(owners, 1)
            ],
            alignments=["right", "left", "right", "right", "right", "right"],
        )
    for owner in owners:
        owner_directories = directories.get(owner.uid, [])
        if not owner_directories:
            continue
        renderer.message("")
        renderer.heading(f"{owner.username}: top directories")
        if metric == "inode_count":
            renderer.table(
                ["Inodes", "Share", "Size", "Path"],
                [
                    (
                        f"{item.inode_count:,}",
                        percent(item.inode_count, owner.inode_count),
                        format_bytes(item.allocated_bytes),
                        item.path,
                    )
                    for item in owner_directories
                ],
                alignments=["right", "right", "right", "left"],
            )
        else:
            renderer.table(
                ["Size", "Share", "Inodes", "Path"],
                [
                    (
                        format_bytes(item.allocated_bytes),
                        percent(item.allocated_bytes, owner.allocated_bytes),
                        f"{item.inode_count:,}",
                        item.path,
                    )
                    for item in owner_directories
                ],
                alignments=["right", "right", "right", "left"],
            )


def _render_files(
    renderer: Renderer,
    records: Sequence[FileRecord],
    *,
    source: str,
    output_format: str,
    stored_limit: int | None,
    stale_enabled: bool,
    reference_epoch: int,
    thresholds: StaleThresholds,
) -> None:
    if output_format == "json":
        files = []
        for item in records:
            value = dataclasses.asdict(item)
            if stale_enabled:
                assessment = assess_staleness(
                    item.modified_epoch,
                    reference_epoch=reference_epoch,
                    thresholds=thresholds,
                )
                value["stale_age_days"] = assessment.age_days
                value["stale_level"] = assessment.level.value
            files.append(value)
        renderer.json(
            {
                "source": source,
                "stored_candidate_limit": stored_limit,
                "staleness": {
                    "enabled": stale_enabled,
                    "reference_epoch": reference_epoch if stale_enabled else None,
                    "thresholds_days": thresholds.as_text() if stale_enabled else None,
                },
                "files": files,
            }
        )
    elif output_format == "tsv":
        headers = [
            "rank",
            "allocated_bytes",
            "apparent_bytes",
            "uid",
            "user",
            "modified_epoch",
        ]
        if stale_enabled:
            headers.extend(["stale_age_days", "stale_level"])
        headers.append("path")
        rows: list[tuple[object, ...]] = []
        for rank, item in enumerate(records, 1):
            values: list[object] = [
                rank,
                item.allocated_bytes,
                item.apparent_bytes,
                item.uid,
                item.username,
                item.modified_epoch,
            ]
            if stale_enabled:
                assessment = assess_staleness(
                    item.modified_epoch,
                    reference_epoch=reference_epoch,
                    thresholds=thresholds,
                )
                values.extend([assessment.age_days, assessment.level.value])
            values.append(item.path)
            rows.append(tuple(values))
        renderer.tsv(headers, rows)
    else:
        renderer.heading("Largest files")
        renderer.key_values(
            [
                ("Source", source),
                (
                    "Stored candidate limit",
                    stored_limit if stored_limit is not None else "exact live scan",
                ),
            ]
        )
        renderer.message("")
        rows: list[tuple[object, ...]] = []
        for rank, item in enumerate(records, 1):
            values: list[object] = [
                rank,
                format_bytes(item.allocated_bytes),
                format_bytes(item.apparent_bytes),
                item.username,
                datetime.fromtimestamp(item.modified_epoch)
                .astimezone()
                .isoformat(timespec="minutes"),
            ]
            if stale_enabled:
                assessment = assess_staleness(
                    item.modified_epoch,
                    reference_epoch=reference_epoch,
                    thresholds=thresholds,
                )
                style = _stale_style(assessment.level)
                values.extend(
                    [
                        TableCell(assessment.warning_label, style),
                        TableCell(item.path, style),
                    ]
                )
            else:
                values.append(item.path)
            rows.append(tuple(values))
        if stale_enabled:
            renderer.table(
                ["Rank", "Allocated", "Apparent", "Owner", "Modified", "Stale", "Path"],
                rows,
                alignments=["right", "right", "right", "left", "left", "right", "left"],
            )
        else:
            renderer.table(
                ["Rank", "Allocated", "Apparent", "Owner", "Modified", "Path"],
                rows,
                alignments=["right", "right", "right", "left", "left", "left"],
            )
