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


def snapshot_command(args: Namespace, config: ConfigRepository) -> int:
    renderer = build_renderer(args)
    profile = load_profile(args, config)
    repository = SnapshotRepository(profile)
    record, summary = repository.create(quiet=args.quiet)
    if not args.quiet:
        renderer.heading("Snapshot saved")
        renderer.key_values(
            [
                ("Profile", profile.name),
                ("Snapshot", record.snapshot_id),
                ("Root", record.root),
                ("Status", "complete" if record.complete else "incomplete"),
                ("Allocated", format_bytes(record.allocated_bytes)),
                ("Files", f"{record.file_count:,}"),
                ("Directories", f"{record.directory_count:,}"),
                ("Filesystem scope", "cross-filesystems" if summary.cross_filesystems else "one filesystem"),
                ("Skipped mounts", f"{summary.skipped_filesystem_count:,}"),
                ("Duration", f"{summary.duration_seconds:.2f}s"),
            ]
        )
    if summary.skipped_filesystem_count:
        noun = "directory" if summary.skipped_filesystem_count == 1 else "directories"
        renderer.warning(
            f"{summary.skipped_filesystem_count} mounted {noun} were skipped because "
            "the scan is limited to one filesystem. Use --cross-filesystems to include them."
        )
        for path in summary.skipped_filesystems[:10]:
            renderer.warning(f"skipped mount: {path}")
    if summary.error_count:
        renderer.warning(
            f"{summary.error_count} path(s) could not be read. "
            "The snapshot is kept as latest-any but does not replace latest."
        )
        for error in summary.errors[:10]:
            renderer.warning(f"{error.path}: {error.message}")
    return 0 if record.complete else 3


def show_command(args: Namespace, config: ConfigRepository) -> int:
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    record = service.resolve(args.snapshot)
    stale_enabled = _stale_enabled(args)
    capacity_policy = _capacity_policy(args, config)
    modified_before_epoch = stale_cutoff_epoch(record.created_epoch, args.stale_only)
    with ExitStack() as stack:
        context = service.open_context(stack, record)
        scope, rows = service.directory_ranking(
            context.connection,
            under=args.under,
            max_depth=args.max_depth,
            top=args.top,
            min_size=parse_size(args.min_size),
            match=args.match,
            metric=args.metric,
            modified_before_epoch=modified_before_epoch,
        )
        assert scope is not None
        scope_metric_value = _directory_metric(scope, args.metric)
        capacity = _capacity_assessment(
            context.connection, context.metadata, capacity_policy
        )
        _warn_age(renderer, record, args.warn_age)
        _warn_capacity(renderer, capacity)
        if args.format == "json":
            renderer.json(
                {
                    "snapshot": _record_json(record),
                    "scope": scope.path,
                    "metric": args.metric,
                    "scope_metric_value": scope_metric_value,
                    "staleness": _staleness_options_json(args, stale_enabled),
                    "capacity": capacity.to_dict() if capacity is not None else None,
                    "entries": [
                        _directory_json(
                            item,
                            scope.path,
                            scope_metric_value,
                            args.metric,
                            reference_epoch=record.created_epoch if stale_enabled else None,
                            thresholds=args.stale_thresholds,
                        )
                        for item in rows
                    ],
                    "filesystem": _filesystem_json(context.metadata, Path(record.root)),
                }
            )
        elif args.format == "tsv":
            headers = [
                "rank",
                "metric",
                "value",
                "allocated_bytes",
                "apparent_bytes",
                "share",
                "files",
                "directories",
                "inodes",
                "capacity_limit_bytes",
                "capacity_used_bytes",
                "capacity_usage_percent",
                "capacity_exceeded",
                "capacity_target",
            ]
            if stale_enabled:
                headers.extend(
                    ["latest_modified_epoch", "stale_age_days", "stale_level"]
                )
            headers.append("path")
            output_rows: list[tuple[object, ...]] = []
            for rank, item in enumerate(rows, 1):
                values: list[object] = [
                    rank,
                    args.metric,
                    _directory_metric(item, args.metric),
                    item.allocated_bytes,
                    item.apparent_bytes,
                    (
                        f"{_directory_metric(item, args.metric) * 100 / scope_metric_value:.4f}"
                        if scope_metric_value
                        else "0"
                    ),
                    item.file_count,
                    item.directory_count,
                    item.inode_count,
                    capacity.limit_bytes if capacity is not None else "",
                    capacity.used_bytes if capacity is not None else "",
                    (f"{capacity.usage_percent:.4f}" if capacity is not None else ""),
                    (str(capacity.exceeded).lower() if capacity is not None else ""),
                    capacity.target if capacity is not None else "",
                ]
                if stale_enabled:
                    assessment = assess_staleness(
                        item.latest_modified_epoch,
                        reference_epoch=record.created_epoch,
                        thresholds=args.stale_thresholds,
                    )
                    values.extend(
                        [
                            item.latest_modified_epoch,
                            assessment.age_days,
                            assessment.level.value,
                        ]
                    )
                values.append(relative_to_scope(item.path, scope.path))
                output_rows.append(tuple(values))
            renderer.tsv(headers, output_rows)
        else:
            _render_snapshot_header(renderer, record, context.metadata, scope, args.metric)
            if capacity is not None:
                _render_capacity_status(renderer, capacity)
                renderer.message("")
            table_rows: list[tuple[object, ...]] = []
            for rank, item in enumerate(rows, 1):
                path = relative_to_scope(item.path, scope.path)
                values: list[object] = [
                    rank,
                    format_bytes(_directory_metric(item, args.metric)),
                    percent(_directory_metric(item, args.metric), scope_metric_value),
                    bar(_directory_metric(item, args.metric), scope_metric_value),
                    f"{item.file_count:,}",
                ]
                if stale_enabled:
                    assessment = assess_staleness(
                        item.latest_modified_epoch,
                        reference_epoch=record.created_epoch,
                        thresholds=args.stale_thresholds,
                    )
                    style = _stale_style(assessment.level)
                    values.extend(
                        [
                            TableCell(assessment.warning_label, style),
                            TableCell(path, style),
                        ]
                    )
                else:
                    values.append(path)
                table_rows.append(tuple(values))
            if stale_enabled:
                renderer.table(
                    ["Rank", "Size", "Share", "Distribution", "Files", "Stale", "Path"],
                    table_rows,
                    alignments=["right", "right", "right", "left", "right", "right", "left"],
                )
                renderer.message(
                    "Stale age is measured at snapshot time from the newest modification "
                    "inside each directory tree."
                )
            else:
                renderer.table(
                    ["Rank", "Size", "Share", "Distribution", "Files", "Path"],
                    table_rows,
                    alignments=["right", "right", "right", "left", "right", "left"],
                )
            if args.users:
                renderer.message("")
                _render_users(
                    renderer,
                    context.connection,
                    service,
                    top=args.user_top,
                    directories_per_user=args.user_dir_top,
                    output_format="table",
                    metric="allocated_bytes",
                )
    return 0

def diff_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    current_record = service.resolve(args.to_snapshot)
    previous_record = service.resolve(args.from_snapshot) if args.from_snapshot else service.previous_of(current_record)
    with ExitStack() as stack:
        previous = service.open_context(stack, previous_record)
        current = service.open_context(stack, current_record)
        _warn_metadata_difference(
            renderer, previous_record, current_record, previous.metadata, current.metadata
        )
        rows = service.diff_ranking(
            previous.connection,
            current.connection,
            under=args.under,
            max_depth=args.max_depth,
            top=args.top,
            min_size=parse_size(args.min_size),
            match=args.match,
            growth_only=args.growth_only,
            shrink_only=args.shrink_only,
            order=args.order,
        )
        scope = normalize_relative_path(args.under)
        if args.format == "json":
            renderer.json(
                {
                    "previous": _record_json(previous_record),
                    "current": _record_json(current_record),
                    "order": args.order,
                    "entries": [
                        {
                            "rank": rank,
                            "path": relative_to_scope(item.path, scope),
                            "previous_bytes": item.previous_bytes,
                            "current_bytes": item.current_bytes,
                            "change_bytes": item.change_bytes,
                        }
                        for rank, item in enumerate(rows, 1)
                    ],
                }
            )
        elif args.format == "tsv":
            renderer.tsv(
                ["rank", "change_bytes", "current_bytes", "previous_bytes", "path"],
                [
                    (
                        rank,
                        item.change_bytes,
                        item.current_bytes,
                        item.previous_bytes,
                        relative_to_scope(item.path, scope),
                    )
                    for rank, item in enumerate(rows, 1)
                ],
            )
        else:
            renderer.heading("Snapshot comparison")
            renderer.key_values(
                [
                    ("Previous", f"{previous_record.snapshot_id}  {previous_record.created_at}"),
                    ("Current", f"{current_record.snapshot_id}  {current_record.created_at}"),
                    ("Scope", scope),
                    ("Order", args.order),
                ]
            )
            renderer.message("")
            rows_for_table = []
            for rank, item in enumerate(rows, 1):
                symbol = "↑" if item.change_bytes > 0 else "↓" if item.change_bytes < 0 else "→"
                rows_for_table.append(
                    (
                        rank,
                        symbol,
                        format_bytes(item.change_bytes, signed=True),
                        format_bytes(item.current_bytes),
                        format_bytes(item.previous_bytes),
                        relative_to_scope(item.path, scope),
                    )
                )
            renderer.table(
                ["Rank", "", "Change", "Current", "Previous", "Path"],
                rows_for_table,
                alignments=["right", "center", "right", "right", "right", "left"],
            )
    return 0


def list_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    index = IndexRepository(ProfilePaths.for_profile(args.profile)).load()
    records = list(reversed(index.snapshots))[: args.top]
    if args.format == "json":
        renderer.json(
            {
                "profile": args.profile,
                "latest_complete": index.latest_complete,
                "latest_any": index.latest_any,
                "snapshots": [_record_json(item) for item in records],
            }
        )
    elif args.format == "tsv":
        renderer.tsv(
            ["snapshot_id", "created_at", "status", "allocated_bytes", "files", "directories", "root", "archived"],
            [
                (
                    item.snapshot_id,
                    item.created_at,
                    "complete" if item.complete else "incomplete",
                    item.allocated_bytes,
                    item.file_count,
                    item.directory_count,
                    item.root,
                    str(item.archived).lower(),
                )
                for item in records
            ],
        )
    else:
        renderer.heading(f"Snapshots — profile: {args.profile}")
        renderer.table(
            ["Created", "Status", "Size", "Files", "Archive", "Snapshot"],
            [
                (
                    item.created_at,
                    "complete" if item.complete else "incomplete",
                    format_bytes(item.allocated_bytes),
                    f"{item.file_count:,}",
                    "yes" if item.archived else "no",
                    item.snapshot_id,
                )
                for item in records
            ],
            alignments=["left", "left", "right", "right", "center", "left"],
        )
    return 0


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


def errors_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    record = service.index.resolve(args.snapshot, include_incomplete=True)
    with ExitStack() as stack:
        context = service.open_context(stack, record)
        rows = context.connection.execute(
            "SELECT path, operation, message FROM scan_errors ORDER BY id LIMIT ?",
            (args.top,),
        ).fetchall()
    values = [(str(row["path"]), str(row["operation"]), str(row["message"])) for row in rows]
    if args.format == "json":
        renderer.json(
            {
                "snapshot": _record_json(record),
                "error_count": record.error_count,
                "errors": [
                    {"path": path, "operation": operation, "message": message}
                    for path, operation, message in values
                ],
            }
        )
    elif args.format == "tsv":
        renderer.tsv(["path", "operation", "message"], values)
    else:
        renderer.heading("Snapshot scan errors")
        renderer.key_values(
            [("Snapshot", record.snapshot_id), ("Total errors", record.error_count)]
        )
        renderer.message("")
        renderer.table(["Path", "Operation", "Message"], values)
    return 0


def check_command(args: Namespace, config: ConfigRepository) -> int:
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    current_record = service.resolve(args.snapshot)
    capacity_policy = _capacity_policy(args, config)
    results: list[CheckResult] = []
    if args.growth_over:
        previous_record = service.previous_of(current_record)
        with ExitStack() as stack:
            previous = service.open_context(stack, previous_record)
            current = service.open_context(stack, current_record)
            scope = normalize_relative_path(args.path_argument)
            previous_value = _directory_value(previous.connection, scope)
            current_value = _directory_value(current.connection, scope)
        change = current_value - previous_value
        threshold = parse_size(args.growth_over)
        results.append(
            CheckResult(
                name="growth",
                passed=change <= threshold,
                observed=format_bytes(change, signed=True),
                threshold=format_bytes(threshold),
                detail=scope,
            )
        )
    if args.disk_usage_over is not None:
        root = Path(current_record.root)
        statvfs = os.statvfs(root)
        total = statvfs.f_blocks * statvfs.f_frsize
        available = statvfs.f_bavail * statvfs.f_frsize
        used_percent = 0.0 if total <= 0 else (total - available) * 100 / total
        threshold_percent = float(args.disk_usage_over)
        results.append(
            CheckResult(
                name="disk_usage",
                passed=used_percent <= threshold_percent,
                observed=f"{used_percent:.1f}%",
                threshold=f"{threshold_percent:.1f}%",
                detail=str(root),
            )
        )
    if args.inode_usage_over is not None:
        root = Path(current_record.root)
        statvfs = os.statvfs(root)
        total = statvfs.f_files
        available = statvfs.f_favail
        used_percent = 0.0 if total <= 0 else (total - available) * 100 / total
        threshold_percent = float(args.inode_usage_over)
        results.append(
            CheckResult(
                name="inode_usage",
                passed=used_percent <= threshold_percent,
                observed=f"{used_percent:.1f}%",
                threshold=f"{threshold_percent:.1f}%",
                detail=str(root),
            )
        )
    if capacity_policy is not None:
        with ExitStack() as stack:
            current = service.open_context(stack, current_record)
            capacity = _capacity_assessment(
                current.connection, current.metadata, capacity_policy
            )
        assert capacity is not None
        results.append(
            CheckResult(
                name="operational_capacity",
                passed=not capacity.exceeded,
                observed=(
                    f"{format_bytes(capacity.used_bytes)} "
                    f"({capacity.usage_percent:.1f}%)"
                ),
                threshold=format_bytes(capacity.limit_bytes),
                detail=capacity.target,
            )
        )
    if not results:
        raise UsageError("check requires at least one threshold option or a configured capacity limit")
    if args.format == "json":
        renderer.json({"passed": all(item.passed for item in results), "checks": [dataclasses.asdict(item) for item in results]})
    elif args.format == "tsv":
        renderer.tsv(
            ["name", "passed", "observed", "threshold", "detail"],
            [(item.name, str(item.passed).lower(), item.observed, item.threshold, item.detail) for item in results],
        )
    else:
        renderer.heading("Capacity checks")
        renderer.table(
            ["Check", "Result", "Observed", "Threshold", "Target"],
            [
                (item.name, "PASS" if item.passed else "ALERT", item.observed, item.threshold, item.detail)
                for item in results
            ],
            alignments=["left", "center", "right", "right", "left"],
        )
    if not all(item.passed for item in results):
        raise ThresholdExceeded("one or more thresholds were exceeded")
    return 0


def compact_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    repository = IndexRepository(paths)
    paths.ensure()
    cutoff = time.time() - parse_duration(args.older_than)
    with ProfileLock(paths.lock, f"compact profile={args.profile}"):
        index = repository.load()
        protected = set(
            item.snapshot_id for item in index.snapshots[-args.keep_latest :]
        ) if args.keep_latest else set()
        if index.latest_complete:
            protected.add(index.latest_complete)
        if index.latest_any:
            protected.add(index.latest_any)
        updated: list[SnapshotIndexRecord] = []
        compacted: list[str] = []
        for record in index.snapshots:
            if record.archived or record.created_epoch > cutoff or record.snapshot_id in protected:
                updated.append(record)
                continue
            source = paths.snapshots / record.filename
            destination = source.with_suffix(source.suffix + ".gz")
            compacted.append(record.snapshot_id)
            if not args.dry_run:
                gzip_snapshot(source, destination)
                source.unlink()
                updated.append(
                    dataclasses.replace(
                        record,
                        filename=destination.name,
                        sha256=sha256_file(destination),
                        archived=True,
                    )
                )
            else:
                updated.append(record)
        if not args.dry_run:
            repository.save(index.with_records(updated))
    renderer.heading("Compaction")
    renderer.key_values(
        [
            ("Profile", args.profile),
            ("Eligible snapshots", len(compacted)),
            ("Mode", "dry-run" if args.dry_run else "completed"),
        ]
    )
    for snapshot_id in compacted:
        renderer.message(snapshot_id)
    return 0


def verify_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    repository = IndexRepository(paths)
    index = repository.load()
    records = list(index.snapshots) if args.all else [repository.resolve(args.snapshot, include_incomplete=True)]
    rows: list[tuple[str, str, str]] = []
    failed = False
    for record in records:
        try:
            file_path = paths.snapshots / record.filename
            digest = sha256_file(file_path)
            if digest != record.sha256:
                raise VerificationError("SHA-256 mismatch")
            with materialized_snapshot(paths, record) as path:
                metadata = validate_database(path)
            if metadata.get("snapshot_id") != record.snapshot_id:
                raise VerificationError("snapshot ID mismatch")
            rows.append((record.snapshot_id, "OK", "verified"))
        except (OSError, VerificationError, SnapshotFormatError) as exc:
            failed = True
            rows.append((record.snapshot_id, "FAILED", str(exc)))
    if args.format == "json":
        renderer.json({"profile": args.profile, "verified": not failed, "results": [{"snapshot": a, "status": b, "detail": c} for a, b, c in rows]})
    elif args.format == "tsv":
        renderer.tsv(["snapshot", "status", "detail"], rows)
    else:
        renderer.heading("Snapshot verification")
        renderer.table(["Snapshot", "Status", "Detail"], rows)
    if failed:
        raise VerificationError("one or more snapshots failed verification")
    return 0


def doctor_command(args: Namespace, config: ConfigRepository) -> int:
    renderer = build_renderer(args)
    profile = config.load_profile(args.profile)
    paths = ProfilePaths.for_profile(args.profile)
    items: list[DoctorItem] = []
    items.append(DoctorItem("Python", "OK", sys.version.split()[0]))
    items.append(DoctorItem("SQLite", "OK", sqlite3.sqlite_version))
    items.append(DoctorItem("Config", "OK" if config.path.exists() else "INFO", str(config.path)))
    try:
        paths.ensure()
        probe = paths.root / ".write-test"
        probe.write_text("ok", encoding="utf-8")
        probe.unlink()
        items.append(DoctorItem("State directory", "OK", str(paths.root)))
    except OSError as exc:
        items.append(DoctorItem("State directory", "FAILED", str(exc)))
    if profile.root.exists() and profile.root.is_dir():
        readable = os.access(profile.root, os.R_OK | os.X_OK)
        items.append(DoctorItem("Profile root", "OK" if readable else "WARN", str(profile.root)))
    else:
        items.append(DoctorItem("Profile root", "FAILED", f"missing: {profile.root}"))
    items.append(
        DoctorItem(
            "Filesystem traversal",
            "INFO",
            "cross-filesystems" if profile.cross_filesystems else "one filesystem",
        )
    )
    active, metadata = inspect_lock(paths.lock)
    items.append(DoctorItem("Lock", "BUSY" if active else "OK", json.dumps(metadata, ensure_ascii=False) if metadata else "not active"))
    if paths.index.exists():
        try:
            index = IndexRepository(paths).load()
            items.append(DoctorItem("Index", "OK", f"{len(index.snapshots)} snapshot(s)"))
            if index.latest_any is not None:
                latest = next(item for item in index.snapshots if item.snapshot_id == index.latest_any)
                items.append(DoctorItem("Bound snapshot root", "OK", latest.root))
        except SnapshotFormatError as exc:
            items.append(DoctorItem("Index", "FAILED", str(exc)))
    else:
        items.append(DoctorItem("Index", "INFO", "not created yet"))
    usage = shutil.disk_usage(paths.root)
    items.append(DoctorItem("State free space", "OK" if usage.free > 100 * 1024 * 1024 else "WARN", format_bytes(usage.free)))
    if args.format == "json":
        renderer.json({"profile": args.profile, "items": [dataclasses.asdict(item) for item in items]})
    elif args.format == "tsv":
        renderer.tsv(["name", "status", "detail"], [(item.name, item.status, item.detail) for item in items])
    else:
        renderer.heading("qdu doctor")
        renderer.table(["Check", "Status", "Detail"], [(item.name, item.status, item.detail) for item in items])
    return 1 if any(item.status == "FAILED" for item in items) else 0


def repair_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    paths.ensure()
    actions: list[str] = []
    with ProfileLock(paths.lock, f"repair profile={args.profile}"):
        for temporary in paths.temporary.glob("*"):
            actions.append(f"remove temporary file: {temporary.name}")
            if not args.dry_run:
                temporary.unlink(missing_ok=True)
        records: list[SnapshotIndexRecord] = []
        for file_path in sorted(paths.snapshots.glob("*.sqlite3*")):
            try:
                archived = file_path.name.endswith(".gz")
                if archived:
                    with tempfile.NamedTemporaryFile(suffix=".sqlite3", delete=False, dir=paths.temporary) as temporary_handle:
                        temporary_path = Path(temporary_handle.name)
                    try:
                        with gzip.open(file_path, "rb") as source, temporary_path.open("wb") as target:
                            shutil.copyfileobj(source, target)
                        metadata = validate_database(temporary_path)
                    finally:
                        temporary_path.unlink(missing_ok=True)
                else:
                    metadata = validate_database(file_path)
                record = _record_from_metadata(metadata, file_path, archived)
                records.append(record)
            except (OSError, VerificationError, ValueError) as exc:
                actions.append(f"skip invalid snapshot {file_path.name}: {exc}")
        new_index = ProfileIndex(
            version=INDEX_VERSION,
            profile=args.profile,
            latest_complete=None,
            latest_any=None,
            snapshots=(),
        ).with_records(records)
        actions.append(f"rebuild index with {len(records)} snapshot(s)")
        if not args.dry_run:
            IndexRepository(paths).save(new_index)
    active, _ = inspect_lock(paths.lock)
    if not active and paths.lock.exists():
        actions.append("clear stale lock metadata")
        if not args.dry_run:
            clear_stale_lock(paths.lock)
    renderer.heading("Repair")
    renderer.key_values([("Profile", args.profile), ("Mode", "dry-run" if args.dry_run else "completed")])
    for action in actions:
        renderer.message(f"- {action}")
    return 0


def unlock_command(args: Namespace) -> int:
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    active, metadata = inspect_lock(paths.lock)
    if active:
        raise BusyError(f"lock is active and cannot be forcibly broken safely: {metadata}")
    removed = clear_stale_lock(paths.lock)
    renderer.message("Stale lock metadata removed." if removed else "No stale lock metadata was present.")
    return 0


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

def _render_snapshot_header(
    renderer: Renderer,
    record: SnapshotIndexRecord,
    metadata: dict[str, str],
    scope: DirectoryRecord,
    metric: str,
) -> None:
    age = max(0, int(time.time()) - record.created_epoch)
    renderer.heading("Disk usage snapshot")
    cross_filesystems = metadata.get("cross_filesystems", "false") == "true"
    skipped_count = int(metadata.get("skipped_filesystem_count", "0"))
    renderer.key_values(
        [
            ("Snapshot", record.snapshot_id),
            ("Captured", f"{record.created_at}  ({format_age(age)} ago)"),
            ("Status", "complete" if record.complete else f"incomplete ({record.error_count} errors)"),
            ("Root", record.root),
            ("Scope", f"{scope.path}  ({format_bytes(_directory_metric(scope, metric))}; {metric})"),
            ("Filesystem scope", "cross-filesystems" if cross_filesystems else "one filesystem"),
            ("Skipped mounts", f"{skipped_count:,}"),
        ]
    )
    filesystem = _filesystem_json(metadata, Path(record.root))
    snapshot_fs = filesystem["snapshot"]
    renderer.message("")
    _render_filesystem_status(renderer, "Filesystem capacity at snapshot time", snapshot_fs)
    current = filesystem.get("current")
    if current:
        renderer.message("")
        _render_filesystem_status(renderer, "Current filesystem capacity", current)
    renderer.message("")


def _filesystem_json(metadata: dict[str, str], root: Path) -> dict[str, object]:
    total = int(metadata.get("filesystem_total_bytes", "0"))
    available = int(metadata.get("filesystem_available_bytes", "0"))
    inode_total = int(metadata.get("filesystem_total_inodes", "0"))
    inode_available = int(metadata.get("filesystem_available_inodes", "0"))
    cross_filesystems = metadata.get("cross_filesystems", "false") == "true"
    stored_paths = tuple(
        path for path in metadata.get("filesystem_paths", ".").splitlines() if path
    ) or (".",)
    result: dict[str, object] = {
        "mode": "cross-filesystems" if cross_filesystems else "one-filesystem",
        "skipped_filesystem_count": int(metadata.get("skipped_filesystem_count", "0")),
        "skipped_filesystems": [
            path for path in metadata.get("skipped_filesystems", "").splitlines() if path
        ],
        "snapshot": {
            "filesystem_count": int(metadata.get("filesystem_count", str(len(stored_paths)))),
            "total_bytes": total,
            "available_bytes": available,
            "used_bytes": max(0, total - available),
            "used_percent": 0.0 if total <= 0 else (total - available) * 100 / total,
            "total_inodes": inode_total,
            "available_inodes": inode_available,
        },
    }
    result["current"] = _current_filesystem_capacity(root, stored_paths)
    return result


def _current_filesystem_capacity(
    root: Path, relative_paths: Sequence[str]
) -> dict[str, int | float] | None:
    seen_devices: set[int] = set()
    total_bytes = 0
    available_bytes = 0
    total_inodes = 0
    available_inodes = 0
    for relative_path in relative_paths:
        path = root if relative_path == "." else root / relative_path
        try:
            device = int(path.stat().st_dev)
            if device in seen_devices:
                continue
            value = os.statvfs(path)
        except OSError:
            continue
        seen_devices.add(device)
        total_bytes += max(0, int(value.f_blocks) * int(value.f_frsize))
        available_bytes += max(0, int(value.f_bavail) * int(value.f_frsize))
        total_inodes += max(0, int(value.f_files))
        available_inodes += max(0, int(value.f_favail))
    if not seen_devices:
        return None
    return {
        "filesystem_count": len(seen_devices),
        "total_bytes": total_bytes,
        "available_bytes": available_bytes,
        "used_bytes": max(0, total_bytes - available_bytes),
        "used_percent": (
            0.0 if total_bytes <= 0 else (total_bytes - available_bytes) * 100 / total_bytes
        ),
        "total_inodes": total_inodes,
        "available_inodes": available_inodes,
    }


def _render_filesystem_status(
    renderer: Renderer, title: str, filesystem: dict[str, object]
) -> None:
    renderer.heading(title)
    total_bytes = int(filesystem["total_bytes"])
    values: list[tuple[str, object]] = [
        ("Filesystems", f"{int(filesystem.get('filesystem_count', 1)):,}"),
    ]
    if total_bytes <= 0:
        values.append(("Capacity", "unavailable"))
    else:
        values.extend(
            [
                ("Used", format_bytes(int(filesystem["used_bytes"]))),
                ("Available", format_bytes(int(filesystem["available_bytes"]))),
                ("Usage", f"{float(filesystem['used_percent']):.1f}%"),
            ]
        )
    renderer.key_values(values)


def _warn_age(renderer: Renderer, record: SnapshotIndexRecord, value: str | None) -> None:
    if value is None:
        return
    age = max(0, int(time.time()) - record.created_epoch)
    threshold = parse_duration(value)
    if age > threshold:
        renderer.warning(f"snapshot is {format_age(age)} old (warning threshold: {value})")


def _warn_metadata_difference(
    renderer: Renderer,
    previous: SnapshotIndexRecord,
    current: SnapshotIndexRecord,
    previous_metadata: dict[str, str],
    current_metadata: dict[str, str],
) -> None:
    if previous.root != current.root:
        renderer.warning(f"snapshot roots differ: {previous.root} vs {current.root}")
    if previous.exclude_hash != current.exclude_hash:
        renderer.warning("exclusion patterns differ between snapshots")
    if previous_metadata.get("cross_filesystems", "false") != current_metadata.get(
        "cross_filesystems", "false"
    ):
        renderer.warning("filesystem traversal modes differ between snapshots")



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


def _profile_capacity_text(profile: object) -> str:
    if profile.capacity_limit_bytes is None:
        return "not set"
    target = profile.capacity_user or "profile root"
    return f"{format_bytes(profile.capacity_limit_bytes)} ({target})"

def _record_json(record: SnapshotIndexRecord | None) -> dict[str, object] | None:
    return record.to_dict() if record is not None else None


def _directory_json(
    item: DirectoryRecord,
    scope: str,
    denominator: int,
    metric: str,
    *,
    reference_epoch: int | None = None,
    thresholds: StaleThresholds,
) -> dict[str, object]:
    metric_value = _directory_metric(item, metric)
    value: dict[str, object] = {
        "path": relative_to_scope(item.path, scope),
        "metric": metric,
        "metric_value": metric_value,
        "allocated_bytes": item.allocated_bytes,
        "apparent_bytes": item.apparent_bytes,
        "share_percent": 0.0 if denominator <= 0 else metric_value * 100 / denominator,
        "file_count": item.file_count,
        "directory_count": item.directory_count,
        "inode_count": item.inode_count,
    }
    if reference_epoch is not None:
        assessment = assess_staleness(
            item.latest_modified_epoch,
            reference_epoch=reference_epoch,
            thresholds=thresholds,
        )
        value.update(
            {
                "latest_modified_epoch": item.latest_modified_epoch,
                "latest_modified_at": datetime.fromtimestamp(item.latest_modified_epoch)
                .astimezone()
                .isoformat(timespec="seconds"),
                "stale_age_days": assessment.age_days,
                "stale_level": assessment.level.value,
            }
        )
    return value


def _stale_enabled(args: Namespace) -> bool:
    return bool(args.stale or args.stale_only is not None)


def _stale_style(level: StaleLevel) -> str | None:
    return {
        StaleLevel.RECENT: None,
        StaleLevel.NOTICE: "blue",
        StaleLevel.WARNING: "yellow",
        StaleLevel.ELEVATED: "orange",
        StaleLevel.CRITICAL: "red",
    }[level]


def _staleness_options_json(args: Namespace, enabled: bool) -> dict[str, object]:
    return {
        "enabled": enabled,
        "minimum_age_days": args.stale_only,
        "thresholds_days": (
            {
                "notice": args.stale_thresholds.notice_days,
                "warning": args.stale_thresholds.warning_days,
                "elevated": args.stale_thresholds.elevated_days,
                "critical": args.stale_thresholds.critical_days,
            }
            if enabled
            else None
        ),
        "reference": "snapshot_captured_at" if enabled else None,
    }


def _directory_metric(item: DirectoryRecord, metric: str) -> int:
    if metric == "apparent_bytes":
        return item.apparent_bytes
    if metric == "inode_count":
        return item.inode_count
    return item.allocated_bytes


def _directory_value(connection: sqlite3.Connection, path: str) -> int:
    row = connection.execute("SELECT allocated_bytes FROM directories WHERE path = ?", (path,)).fetchone()
    return int(row[0]) if row is not None else 0


def _record_from_metadata(metadata: dict[str, str], file_path: Path, archived: bool) -> SnapshotIndexRecord:
    return SnapshotIndexRecord(
        snapshot_id=metadata["snapshot_id"],
        filename=file_path.name,
        created_epoch=int(metadata["created_epoch"]),
        created_at=metadata["created_at"],
        root=metadata["root"],
        host=metadata["host"],
        complete=metadata.get("complete") == "true",
        error_count=int(metadata.get("error_count", "0")),
        allocated_bytes=int(metadata["allocated_bytes"]),
        apparent_bytes=int(metadata["apparent_bytes"]),
        directory_count=int(metadata["directory_count"]),
        file_count=int(metadata["file_count"]),
        inode_count=int(metadata["inode_count"]),
        collect_users=metadata.get("collect_users") == "true",
        large_file_limit=int(metadata.get("large_file_limit", "0")),
        exclude_hash=metadata.get("exclude_hash", ""),
        sha256=sha256_file(file_path),
        archived=archived,
    )


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
