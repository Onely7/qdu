"""Operational maintenance and integrity-check commands."""

from __future__ import annotations

import contextlib
import dataclasses
import os
import shutil
import sqlite3
import sys
import time
from argparse import Namespace
from contextlib import ExitStack
from pathlib import Path

from qdu.commands.capacity_support import _capacity_assessment, _capacity_policy
from qdu.commands.context import build_renderer
from qdu.commands.schedule import scheduled_entry
from qdu.config import ConfigRepository
from qdu.errors import (
    SnapshotFormatError,
    ThresholdExceeded,
    UsageError,
    VerificationError,
)
from qdu.locking import ProfileLock, describe_holder, inspect_lock
from qdu.models import (
    CheckResult,
    DoctorItem,
    ProfileConfig,
    ProfileIndex,
    SnapshotIndexRecord,
)
from qdu.query import (
    SnapshotQueryService,
    directory_allocation,
    normalize_relative_path,
)
from qdu.render import TableCell
from qdu.repository import archive_snapshots
from qdu.storage import (
    INDEX_VERSION,
    IndexRepository,
    ProfilePaths,
    decompressed_snapshot,
    materialized_snapshot,
    sha256_file,
    validate_database,
)
from qdu.units import (
    format_age,
    format_bytes,
    format_local_time,
    parse_duration,
    parse_size,
)


def check_command(args: Namespace, config: ConfigRepository) -> int:
    """Evaluate automation-oriented usage and freshness thresholds."""
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
            scope = normalize_relative_path(args.under)
            previous_value = directory_allocation(previous.connection, scope)
            current_value = directory_allocation(current.connection, scope)
        if previous_value is None and current_value is None:
            raise UsageError(f"directory is absent from both snapshots: {args.under}")
        # A directory present on one side only was created or removed.
        change = (current_value or 0) - (previous_value or 0)
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
        if capacity is None:
            raise SnapshotFormatError("capacity policy produced no assessment")
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
    if args.snapshot_age_over is not None:
        maximum_age = parse_duration(args.snapshot_age_over)
        age = max(0, int(time.time()) - current_record.created_epoch)
        results.append(
            CheckResult(
                name="snapshot_age",
                passed=age <= maximum_age,
                observed=format_age(age),
                threshold=format_age(maximum_age),
                detail=current_record.snapshot_id,
            )
        )
    if not results:
        raise UsageError(
            "check requires at least one threshold option or a configured capacity limit"
        )
    if args.format == "json":
        renderer.json(
            {
                "passed": all(item.passed for item in results),
                "checks": [dataclasses.asdict(item) for item in results],
            }
        )
    elif args.format == "tsv":
        renderer.tsv(
            ["name", "passed", "observed", "threshold", "detail"],
            [
                (
                    item.name,
                    str(item.passed).lower(),
                    item.observed,
                    item.threshold,
                    item.detail,
                )
                for item in results
            ],
        )
    else:
        renderer.heading("Checks")
        renderer.table(
            ["Check", "Result", "Observed", "Threshold", "Target"],
            [
                (
                    item.name,
                    "PASS" if item.passed else "ALERT",
                    item.observed,
                    item.threshold,
                    item.detail,
                )
                for item in results
            ],
            alignments=["left", "center", "right", "right", "left"],
        )
    if not all(item.passed for item in results):
        raise ThresholdExceeded("one or more thresholds were exceeded")
    return 0


def compact_command(args: Namespace) -> int:
    """Compress eligible snapshots while preserving configured retention."""
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    repository = IndexRepository(paths)
    paths.ensure()
    older_than_seconds = parse_duration(args.older_than)
    with ProfileLock(paths.lock, f"compact profile={args.profile}"):
        compacted = archive_snapshots(
            paths,
            repository,
            older_than_seconds=older_than_seconds,
            keep_latest=args.keep_latest,
            dry_run=args.dry_run,
        )
    if not compacted:
        renderer.message(
            f"Nothing to compress: no snapshot is older than {args.older_than} "
            f"apart from the {args.keep_latest} newest."
        )
    elif args.dry_run:
        renderer.message(f"Would compress {len(compacted)} snapshot(s):")
    else:
        renderer.message(
            f"Compressed {len(compacted)} snapshot(s); the profile's snapshots "
            f"now use {format_bytes(_stored_total(paths))}:"
        )
    for snapshot_id in compacted:
        renderer.message(f"  {snapshot_id}")
    return 0


def _stored_total(paths: ProfilePaths) -> int:
    total = 0
    for path in paths.snapshots.iterdir():
        with contextlib.suppress(OSError):
            total += path.stat().st_size
    return total


def verify_command(args: Namespace) -> int:
    """Verify indexed snapshot files and their database integrity."""
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    repository = IndexRepository(paths)
    index = repository.load()
    records = (
        list(index.snapshots)
        if args.all
        else [repository.resolve(args.snapshot, include_incomplete=True)]
    )
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
        renderer.json(
            {
                "profile": args.profile,
                "verified": not failed,
                "results": [
                    {"snapshot": a, "status": b, "detail": c} for a, b, c in rows
                ],
            }
        )
    elif args.format == "tsv":
        renderer.tsv(["snapshot", "status", "detail"], rows)
    else:
        renderer.heading("Snapshot verification")
        renderer.table(["Snapshot", "Status", "Detail"], rows)
    if failed:
        raise VerificationError("one or more snapshots failed verification")
    return 0


def doctor_command(args: Namespace, config: ConfigRepository) -> int:
    """Diagnose profile configuration, storage, schedule, and lock health."""
    renderer = build_renderer(args)
    profile = config.load_profile(args.profile)
    paths = ProfilePaths.for_profile(args.profile)
    items: list[DoctorItem] = []
    items.append(DoctorItem("Python", "OK", sys.version.split()[0]))
    items.append(DoctorItem("SQLite", "OK", sqlite3.sqlite_version))
    items.append(
        DoctorItem("Config", "OK", str(config.path))
        if config.path.exists()
        else DoctorItem(
            "Config", "INFO", f"{config.path} not created; built-in defaults apply"
        )
    )
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
        items.append(
            DoctorItem("Profile root", "OK" if readable else "WARN", str(profile.root))
        )
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
    items.append(
        DoctorItem(
            "Lock",
            "BUSY",
            describe_holder(metadata) if metadata else "held",
        )
        if active
        else DoctorItem("Lock", "OK", "not held")
    )
    items.extend(_snapshot_items(paths, profile))
    items.append(_schedule_item(args.profile))
    usage = shutil.disk_usage(paths.root)
    items.append(
        DoctorItem(
            "State free space",
            "OK" if usage.free > 100 * 1024 * 1024 else "WARN",
            format_bytes(usage.free),
        )
    )
    if args.format == "json":
        renderer.json(
            {
                "profile": args.profile,
                "items": [dataclasses.asdict(item) for item in items],
            }
        )
    elif args.format == "tsv":
        renderer.tsv(
            ["name", "status", "detail"],
            [(item.name, item.status, item.detail) for item in items],
        )
    else:
        renderer.heading(f"qdu doctor — profile: {args.profile}")
        renderer.table(
            ["Check", "Status", "Detail"],
            [
                (
                    item.name,
                    TableCell(item.status, _STATUS_STYLES.get(item.status)),
                    item.detail,
                )
                for item in items
            ],
        )
    return 1 if any(item.status == "FAILED" for item in items) else 0


_STATUS_STYLES = {"OK": "green", "WARN": "yellow", "BUSY": "yellow", "FAILED": "red"}


def _snapshot_items(paths: ProfilePaths, profile: ProfileConfig) -> list[DoctorItem]:
    if not paths.index.exists():
        return [DoctorItem("Index", "INFO", "no snapshots yet; run 'qdu snapshot'")]
    try:
        index = IndexRepository(paths).load()
    except SnapshotFormatError as exc:
        return [DoctorItem("Index", "FAILED", f"{exc}; run 'qdu repair'")]
    stored = 0
    missing = 0
    for record in index.snapshots:
        try:
            stored += (paths.snapshots / record.filename).stat().st_size
        except OSError:
            missing += 1
    items = [
        DoctorItem(
            "Index",
            "FAILED" if missing else "OK",
            f"{len(index.snapshots):,} snapshot(s) use {format_bytes(stored)}"
            + (f"; {missing:,} file(s) missing, run 'qdu repair'" if missing else ""),
        )
    ]
    if not index.snapshots:
        return items
    newest = index.snapshots[-1]
    age = format_age(int(time.time()) - newest.created_epoch)
    items.append(
        DoctorItem(
            "Latest snapshot",
            "OK" if newest.complete else "WARN",
            f"{format_local_time(newest.created_epoch)} ({age} ago), "
            + (
                "complete"
                if newest.complete
                else f"incomplete with {newest.error_count:,} error(s); see 'qdu errors'"
            ),
        )
    )
    # A snapshot refuses to run when the root differs from the snapshots' root.
    root = profile.root.expanduser().resolve()
    if Path(newest.root) == root:
        items.append(DoctorItem("Snapshot root", "OK", newest.root))
    else:
        items.append(
            DoctorItem(
                "Snapshot root",
                "FAILED",
                f"snapshots are of {newest.root}, but the profile scans {root}, "
                "so 'qdu snapshot' refuses to run",
            )
        )
    return items


def _schedule_item(profile: str) -> DoctorItem:
    try:
        entry = scheduled_entry(profile)
    except UsageError as exc:
        return DoctorItem("Schedule", "INFO", str(exc))
    if entry is None:
        return DoctorItem("Schedule", "INFO", "not scheduled; see 'qdu schedule'")
    return DoctorItem("Schedule", "OK", f"cron {' '.join(entry.split()[:5])}")


def repair_command(args: Namespace) -> int:
    """Rebuild a profile index from valid snapshot databases."""
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    paths.ensure()
    actions: list[str] = []
    with ProfileLock(paths.lock, f"repair profile={args.profile}"):
        for temporary in paths.temporary.glob("*"):
            actions.append(f"remove temporary file: {temporary.name}")
            if not args.dry_run:
                temporary.unlink(missing_ok=True)
        records: dict[str, SnapshotIndexRecord] = {}
        for file_path in sorted(paths.snapshots.glob("*.sqlite3*")):
            try:
                archived = file_path.name.endswith(".gz")
                if archived:
                    with decompressed_snapshot(file_path) as temporary_path:
                        metadata = validate_database(temporary_path)
                else:
                    metadata = validate_database(file_path)
                record = _record_from_metadata(metadata, file_path, archived)
            except (
                OSError,
                SnapshotFormatError,
                VerificationError,
                ValueError,
            ) as exc:
                actions.append(f"skip invalid snapshot {file_path.name}: {exc}")
                continue
            kept = records.get(record.snapshot_id)
            if kept is not None:
                # An interrupted compaction can leave "<id>.sqlite3" next to
                # "<id>.sqlite3.gz"; the sorted walk sees the uncompressed copy
                # first. Keep it and remove only that redundant archive.
                if record.archived and record.filename == f"{kept.filename}.gz":
                    actions.append(f"remove duplicate archive: {record.filename}")
                    if not args.dry_run:
                        (paths.snapshots / record.filename).unlink(missing_ok=True)
                else:
                    actions.append(
                        f"skip duplicate snapshot {record.filename}: "
                        f"{record.snapshot_id} is already indexed"
                    )
                continue
            records[record.snapshot_id] = record
        new_index = ProfileIndex(
            version=INDEX_VERSION,
            profile=args.profile,
            latest_complete=None,
            latest_any=None,
            snapshots=(),
        ).with_records(records.values())
        actions.append(f"rebuild index with {len(records)} snapshot(s)")
        if not args.dry_run:
            IndexRepository(paths).save(new_index)
    renderer.heading(
        f"Repair — profile: {args.profile}"
        + (" (dry run; nothing was changed)" if args.dry_run else "")
    )
    for action in actions:
        renderer.message(f"- {action}")
    return 0


def _record_from_metadata(
    metadata: dict[str, str], file_path: Path, archived: bool
) -> SnapshotIndexRecord:
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
