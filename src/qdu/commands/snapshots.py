"""Commands for creating, displaying, and comparing snapshots."""

from __future__ import annotations

import os
import sys
import time
from argparse import Namespace
from collections.abc import Sequence
from contextlib import ExitStack
from datetime import datetime
from functools import partial
from pathlib import Path

from qdu.commands.capacity_support import (
    _capacity_assessment,
    _capacity_policy,
    _render_capacity_status,
    _warn_capacity,
)
from qdu.commands.context import build_renderer, load_profile
from qdu.config import ConfigRepository
from qdu.errors import UsageError
from qdu.models import (
    DiffRecord,
    DirectoryRecord,
    SnapshotIndexRecord,
)
from qdu.query import (
    SnapshotQueryService,
    normalize_relative_path,
    relative_to_scope,
)
from qdu.render import ProgressLine, Renderer, TableCell, bar, percent
from qdu.repository import SnapshotOutcome, SnapshotRepository
from qdu.scanner import ScanProgress
from qdu.staleness import (
    StaleLevel,
    StaleThresholds,
    assess_staleness,
    stale_cutoff_epoch,
)
from qdu.storage import (
    IndexRepository,
    ProfilePaths,
)
from qdu.units import (
    format_age,
    format_bytes,
    format_local_time,
    parse_duration,
    parse_size,
)


def snapshot_command(args: Namespace, config: ConfigRepository) -> int:
    """Scan a profile root and persist a new snapshot."""
    renderer = build_renderer(args)
    profile = load_profile(args, config)
    repository = SnapshotRepository(profile)
    progress_line = (
        ProgressLine(sys.stderr) if not args.quiet and sys.stderr.isatty() else None
    )
    try:
        outcome = repository.create(
            progress=(
                partial(_show_scan_progress, progress_line)
                if progress_line is not None
                else None
            )
        )
    finally:
        if progress_line is not None:
            progress_line.clear()
    record, summary = outcome.record, outcome.summary
    if not args.quiet:
        stored = _stored_bytes(repository.paths, record)
        values: list[tuple[str, object]] = [
            ("Profile", profile.name),
            ("Snapshot", record.snapshot_id),
            ("Root", record.root),
            (
                "Status",
                "complete"
                if record.complete
                else f"incomplete, {record.error_count:,} unreadable path(s)",
            ),
            (
                "Usage",
                f"{format_bytes(record.allocated_bytes)} in "
                f"{_count(record.file_count, 'file')} and "
                f"{_count(record.directory_count, 'directory', 'directories')}",
            ),
            ("Duration", format_age(round(summary.duration_seconds))),
            ("Stored", _format_stored(stored)),
        ]
        if summary.cross_filesystems:
            values.append(("Filesystems", "crossed into other mounts"))
        if outcome.removed or outcome.archived:
            values.append(("Retention", _retention_text(outcome)))
        renderer.heading("Snapshot saved")
        renderer.key_values(values)
    if summary.skipped_filesystem_count:
        count = summary.skipped_filesystem_count
        mounts = _count(count, "mounted directory", "mounted directories")
        renderer.warning(
            f"{mounts} {'was' if count == 1 else 'were'} skipped because the scan "
            "is limited to one filesystem. Use --cross-filesystems to include them."
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


def _retention_text(outcome: SnapshotOutcome) -> str:
    parts = []
    if outcome.removed:
        parts.append(f"removed {len(outcome.removed)} old snapshot(s)")
    if outcome.archived:
        parts.append(f"compressed {len(outcome.archived)} older snapshot(s)")
    return ", ".join(parts)


def _show_scan_progress(line: ProgressLine, progress: ScanProgress) -> None:
    elapsed = format_age(int(progress.elapsed_seconds))
    if progress.phase == "writing":
        line.update(
            f"Writing snapshot ({progress.entries:,} entries scanned in {elapsed})"
        )
        return
    text = (
        f"Scanning {progress.entries:,} entries in {progress.directories:,} "
        f"directories, {format_bytes(progress.allocated_bytes)}, {elapsed}"
    )
    if progress.errors:
        text += f", {progress.errors:,} errors"
    line.update(text, progress.current_path)


def show_command(args: Namespace, config: ConfigRepository) -> int:
    """Render directory usage from one snapshot."""
    renderer = build_renderer(args)
    if args.users and args.format == "tsv":
        raise UsageError(
            "--users cannot be combined with --format tsv; "
            "use 'qdu users --format tsv' for owner usage"
        )
    service = SnapshotQueryService(args.profile)
    record = service.resolve(args.snapshot)
    stale_enabled = _stale_enabled(args)
    capacity_policy = _capacity_policy(args, config)
    modified_before_epoch = stale_cutoff_epoch(record.created_epoch, args.stale_only)
    with ExitStack() as stack:
        context = service.open_context(stack, record)
        if args.users and context.metadata.get("collect_users") != "true":
            raise UsageError(
                "this snapshot has no owner statistics; use 'qdu snapshot --with-users'"
            )
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
        scope_metric_value = _directory_metric(scope, args.metric)
        try:
            capacity = _capacity_assessment(
                context.connection, context.metadata, capacity_policy
            )
        except UsageError as exc:
            # The ranking stays useful when the snapshot cannot answer the
            # capacity question; 'qdu check' still fails on it.
            renderer.warning(f"capacity limit not evaluated: {exc}")
            capacity = None
        _warn_age(renderer, record, args.warn_age)
        _warn_capacity(renderer, capacity)
        if args.format == "json":
            payload: dict[str, object] = {
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
            if args.users:
                # Import lazily because analysis commands call show_command.
                from qdu.commands.analysis import _owners_json

                owners, owner_directories, _ = service.owners(
                    context.connection,
                    top=args.user_top,
                    directories_per_user=args.user_dir_top,
                )
                payload["users"] = _owners_json(owners, owner_directories)
            renderer.json(payload)
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
            _render_snapshot_header(
                renderer, record, context.metadata, scope, args.metric
            )
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
                    alignments=[
                        "right",
                        "right",
                        "right",
                        "left",
                        "right",
                        "right",
                        "left",
                    ],
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
                # Import lazily because analysis commands call show_command for drill-down.
                from qdu.commands.analysis import _render_users

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
    """Render directory usage changes between two snapshots."""
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    current_record = service.resolve(args.to_snapshot)
    previous_record = (
        service.resolve(args.from_snapshot)
        if args.from_snapshot
        else service.previous_of(current_record)
    )
    with ExitStack() as stack:
        previous = service.open_context(stack, previous_record)
        current = service.open_context(stack, current_record)
        _warn_metadata_difference(
            renderer,
            previous_record,
            current_record,
            previous.metadata,
            current.metadata,
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
            metric=args.metric,
        )
        scope = normalize_relative_path(args.under)
        if args.format == "json":
            renderer.json(
                {
                    "previous": _record_json(previous_record),
                    "current": _record_json(current_record),
                    "metric": args.metric,
                    "order": args.order,
                    "entries": [
                        {
                            "rank": rank,
                            "path": relative_to_scope(item.path, scope),
                            "previous": item.previous,
                            "current": item.current,
                            "change": item.change,
                            "change_percent": item.change_percent,
                        }
                        for rank, item in enumerate(rows, 1)
                    ],
                }
            )
        elif args.format == "tsv":
            renderer.tsv(
                [
                    "rank",
                    "metric",
                    "change",
                    "change_percent",
                    "current",
                    "previous",
                    "path",
                ],
                [
                    (
                        rank,
                        args.metric,
                        item.change,
                        ""
                        if item.change_percent is None
                        else f"{item.change_percent:.4f}",
                        item.current,
                        item.previous,
                        relative_to_scope(item.path, scope),
                    )
                    for rank, item in enumerate(rows, 1)
                ],
            )
        else:
            span = max(0, current_record.created_epoch - previous_record.created_epoch)
            renderer.heading("Changes")
            renderer.key_values(
                [
                    (
                        "From",
                        f"{format_local_time(previous_record.created_epoch)}  "
                        f"{previous_record.snapshot_id}",
                    ),
                    (
                        "To",
                        f"{format_local_time(current_record.created_epoch)}  "
                        f"{current_record.snapshot_id}  ({format_age(span)} later)",
                    ),
                    ("Scope", scope),
                    (
                        "Metric",
                        f"{_METRIC_LABELS[args.metric]}, ordered by {args.order}",
                    ),
                ]
            )
            renderer.message("")
            rows_for_table = []
            for rank, item in enumerate(rows, 1):
                symbol = "↑" if item.change > 0 else "↓" if item.change < 0 else "→"
                rows_for_table.append(
                    (
                        rank,
                        symbol,
                        _format_metric(item.change, args.metric, signed=True),
                        _format_change_percent(item),
                        _format_metric(item.current, args.metric),
                        _format_metric(item.previous, args.metric),
                        relative_to_scope(item.path, scope),
                    )
                )
            renderer.table(
                ["Rank", "", "Change", "Change %", "Current", "Previous", "Path"],
                rows_for_table,
                empty_message="No changes in this scope.",
                alignments=[
                    "right",
                    "center",
                    "right",
                    "right",
                    "right",
                    "right",
                    "left",
                ],
            )
    return 0


def _format_metric(value: int, metric: str, *, signed: bool = False) -> str:
    if metric != "inode_count":
        return format_bytes(value, signed=signed)
    return f"{value:+,}" if signed and value else f"{value:,}"


def _format_change_percent(record: DiffRecord) -> str:
    if record.change_percent is None:
        return "new" if record.current else ""
    return f"{record.change_percent:+.1f}%"


def list_command(args: Namespace) -> int:
    """List snapshots recorded for a profile with their on-disk sizes."""
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    index = IndexRepository(paths).load()
    stored = {item.snapshot_id: _stored_bytes(paths, item) for item in index.snapshots}
    stored_total = sum(size for size in stored.values() if size is not None)
    records = list(reversed(index.snapshots))[: args.top]
    if args.format == "json":
        renderer.json(
            {
                "profile": args.profile,
                "latest_complete": index.latest_complete,
                "latest_any": index.latest_any,
                "stored_bytes_total": stored_total,
                "snapshots": [
                    {**item.to_dict(), "stored_bytes": stored[item.snapshot_id]}
                    for item in records
                ],
            }
        )
    elif args.format == "tsv":
        renderer.tsv(
            [
                "snapshot_id",
                "created_at",
                "status",
                "allocated_bytes",
                "files",
                "directories",
                "root",
                "archived",
                "stored_bytes",
            ],
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
                    _optional(stored[item.snapshot_id]),
                )
                for item in records
            ],
        )
    else:
        renderer.heading(f"Snapshots — profile: {args.profile}")
        renderer.table(
            ["Created", "Status", "Size", "Files", "Archive", "Stored", "Snapshot"],
            [
                (
                    format_local_time(item.created_epoch),
                    "complete" if item.complete else "incomplete",
                    format_bytes(item.allocated_bytes),
                    f"{item.file_count:,}",
                    "yes" if item.archived else "no",
                    _format_stored(stored[item.snapshot_id]),
                    item.snapshot_id,
                )
                for item in records
            ],
            alignments=["left", "left", "right", "right", "center", "right", "left"],
        )
        renderer.message(
            f"{len(index.snapshots):,} snapshot(s) use {format_bytes(stored_total)} "
            f"in {paths.snapshots}"
        )
    return 0


def _stored_bytes(paths: ProfilePaths, record: SnapshotIndexRecord) -> int | None:
    try:
        return (paths.snapshots / record.filename).stat().st_size
    except OSError:
        return None


def _count(number: int, singular: str, plural: str | None = None) -> str:
    """Write a count with the singular or plural form of its noun."""
    return f"{number:,} {singular if number == 1 else plural or singular + 's'}"


def _format_stored(size: int | None) -> str:
    return "missing" if size is None else format_bytes(size)


def _optional(value: object | None) -> object:
    return "" if value is None else value


def errors_command(args: Namespace) -> int:
    """Render scan errors stored in a snapshot."""
    renderer = build_renderer(args)
    service = SnapshotQueryService(args.profile)
    record = service.index.resolve(args.snapshot, include_incomplete=True)
    with ExitStack() as stack:
        context = service.open_context(stack, record)
        rows = context.connection.execute(
            "SELECT path, operation, message FROM scan_errors ORDER BY id LIMIT ?",
            (args.top,),
        ).fetchall()
    values = [
        (str(row["path"]), str(row["operation"]), str(row["message"])) for row in rows
    ]
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
        renderer.table(
            ["Path", "Operation", "Message"], values, empty_message="No scan errors."
        )
    return 0


def _render_snapshot_header(
    renderer: Renderer,
    record: SnapshotIndexRecord,
    metadata: dict[str, str],
    scope: DirectoryRecord,
    metric: str,
    *,
    heading: str = "Disk usage",
) -> None:
    values: list[tuple[str, object]] = [
        ("Root", record.root),
        ("Snapshot", _snapshot_text(record)),
    ]
    if not record.complete:
        values.append(
            (
                "Status",
                f"incomplete, {record.error_count:,} unreadable path(s); "
                "see 'qdu errors'",
            )
        )
    values.append(
        (
            "Scope",
            f"{scope.path}  {_format_metric(_directory_metric(scope, metric), metric)} "
            f"{_METRIC_LABELS[metric]}",
        )
    )
    filesystem = _filesystem_json(metadata, Path(record.root))
    if metric == "inode_count":
        values.append(("Inodes", _inode_text(filesystem)))
    else:
        values.append(("Disk", _filesystem_text(filesystem)))
    skipped = int(metadata.get("skipped_filesystem_count", "0"))
    if skipped:
        values.append(
            ("Mounts", f"{skipped:,} skipped; scan with --cross-filesystems to include")
        )
    renderer.heading(heading)
    renderer.key_values(values)
    renderer.message("")


def _snapshot_text(record: SnapshotIndexRecord) -> str:
    """Describe when a snapshot was taken, followed by its ID."""
    age = max(0, int(time.time()) - record.created_epoch)
    return (
        f"{format_local_time(record.created_epoch)} ({format_age(age)} ago)  "
        f"{record.snapshot_id}"
    )


_METRIC_LABELS = {
    "allocated_bytes": "allocated",
    "apparent_bytes": "apparent",
    "inode_count": "inodes",
}


def _filesystem_text(filesystem: dict[str, object]) -> str:
    current = filesystem.get("current")
    usage = current if isinstance(current, dict) else filesystem["snapshot"]
    if not isinstance(usage, dict) or int(usage["total_bytes"]) <= 0:
        return "capacity unavailable"
    text = (
        f"{float(usage['used_percent']):.1f}% used, "
        f"{format_bytes(int(usage['available_bytes']))} available of "
        f"{format_bytes(int(usage['total_bytes']))}"
    )
    if usage is not current:
        text += " at snapshot time"
    count = int(usage.get("filesystem_count", 1))
    if count > 1:
        text += f" across {count} filesystems"
    return text


def _inode_text(filesystem: dict[str, object]) -> str:
    current = filesystem.get("current")
    usage = current if isinstance(current, dict) else filesystem["snapshot"]
    if not isinstance(usage, dict) or int(usage["total_inodes"]) <= 0:
        return "inode counts unavailable"
    total = int(usage["total_inodes"])
    available = int(usage["available_inodes"])
    text = (
        f"{(total - available) * 100 / total:.1f}% used, "
        f"{available:,} available of {total:,}"
    )
    if usage is not current:
        text += " at snapshot time"
    return text


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
            path
            for path in metadata.get("skipped_filesystems", "").splitlines()
            if path
        ],
        "snapshot": {
            "filesystem_count": int(
                metadata.get("filesystem_count", str(len(stored_paths)))
            ),
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
            0.0
            if total_bytes <= 0
            else (total_bytes - available_bytes) * 100 / total_bytes
        ),
        "total_inodes": total_inodes,
        "available_inodes": available_inodes,
    }


def _warn_age(
    renderer: Renderer, record: SnapshotIndexRecord, value: str | None
) -> None:
    if value is None:
        return
    age = max(0, int(time.time()) - record.created_epoch)
    threshold = parse_duration(value)
    if age > threshold:
        renderer.warning(
            f"snapshot is {format_age(age)} old (warning threshold: {value})"
        )


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
