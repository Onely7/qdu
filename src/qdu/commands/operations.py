"""Operational maintenance and integrity-check commands."""

from __future__ import annotations

import dataclasses
import gzip
import json
import os
import shutil
import sqlite3
import sys
import tempfile
import time
from argparse import Namespace
from contextlib import ExitStack
from pathlib import Path

from qdu.commands.capacity_support import _capacity_assessment, _capacity_policy
from qdu.commands.context import build_renderer
from qdu.config import ConfigRepository
from qdu.errors import (
    BusyError,
    SnapshotFormatError,
    ThresholdExceeded,
    UsageError,
    VerificationError,
)
from qdu.locking import ProfileLock, clear_stale_lock, inspect_lock
from qdu.models import (
    CheckResult,
    DoctorItem,
    ProfileIndex,
    SnapshotIndexRecord,
)
from qdu.query import (
    SnapshotQueryService,
    normalize_relative_path,
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
from qdu.units import format_bytes, parse_duration, parse_size


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
        renderer.heading("Capacity checks")
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
    cutoff = time.time() - parse_duration(args.older_than)
    with ProfileLock(paths.lock, f"compact profile={args.profile}"):
        index = repository.load()
        protected = (
            {item.snapshot_id for item in index.snapshots[-args.keep_latest :]}
            if args.keep_latest
            else set()
        )
        if index.latest_complete:
            protected.add(index.latest_complete)
        if index.latest_any:
            protected.add(index.latest_any)
        updated: list[SnapshotIndexRecord] = []
        compacted: list[str] = []
        for record in index.snapshots:
            if (
                record.archived
                or record.created_epoch > cutoff
                or record.snapshot_id in protected
            ):
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
    """Diagnose profile configuration, storage, and lock health."""
    renderer = build_renderer(args)
    profile = config.load_profile(args.profile)
    paths = ProfilePaths.for_profile(args.profile)
    items: list[DoctorItem] = []
    items.append(DoctorItem("Python", "OK", sys.version.split()[0]))
    items.append(DoctorItem("SQLite", "OK", sqlite3.sqlite_version))
    items.append(
        DoctorItem("Config", "OK" if config.path.exists() else "INFO", str(config.path))
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
            "BUSY" if active else "OK",
            json.dumps(metadata, ensure_ascii=False) if metadata else "not active",
        )
    )
    if paths.index.exists():
        try:
            index = IndexRepository(paths).load()
            items.append(
                DoctorItem("Index", "OK", f"{len(index.snapshots)} snapshot(s)")
            )
            if index.latest_any is not None:
                latest = next(
                    item
                    for item in index.snapshots
                    if item.snapshot_id == index.latest_any
                )
                items.append(DoctorItem("Bound snapshot root", "OK", latest.root))
        except SnapshotFormatError as exc:
            items.append(DoctorItem("Index", "FAILED", str(exc)))
    else:
        items.append(DoctorItem("Index", "INFO", "not created yet"))
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
        renderer.heading("qdu doctor")
        renderer.table(
            ["Check", "Status", "Detail"],
            [(item.name, item.status, item.detail) for item in items],
        )
    return 1 if any(item.status == "FAILED" for item in items) else 0


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
        records: list[SnapshotIndexRecord] = []
        for file_path in sorted(paths.snapshots.glob("*.sqlite3*")):
            try:
                archived = file_path.name.endswith(".gz")
                if archived:
                    with tempfile.NamedTemporaryFile(
                        suffix=".sqlite3", delete=False, dir=paths.temporary
                    ) as temporary_handle:
                        temporary_path = Path(temporary_handle.name)
                    try:
                        with (
                            gzip.open(file_path, "rb") as source,
                            temporary_path.open("wb") as target,
                        ):
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
    renderer.key_values(
        [
            ("Profile", args.profile),
            ("Mode", "dry-run" if args.dry_run else "completed"),
        ]
    )
    for action in actions:
        renderer.message(f"- {action}")
    return 0


def unlock_command(args: Namespace) -> int:
    """Remove a stale profile lock after validating its ownership state."""
    renderer = build_renderer(args)
    paths = ProfilePaths.for_profile(args.profile)
    active, metadata = inspect_lock(paths.lock)
    if active:
        raise BusyError(
            f"lock is active and cannot be forcibly broken safely: {metadata}"
        )
    removed = clear_stale_lock(paths.lock)
    renderer.message(
        "Stale lock metadata removed."
        if removed
        else "No stale lock metadata was present."
    )
    return 0


def _directory_value(connection: sqlite3.Connection, path: str) -> int:
    row = connection.execute(
        "SELECT allocated_bytes FROM directories WHERE path = ?", (path,)
    ).fetchone()
    return int(row[0]) if row is not None else 0


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
