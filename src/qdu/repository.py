"""Orchestrate scanning, snapshot finalization, indexing, and retention."""

from __future__ import annotations

import os
import sqlite3
import time
import uuid
from pathlib import Path

from qdu.errors import UsageError
from qdu.locking import ProfileLock
from qdu.models import ProfileConfig, SnapshotIndexRecord, SnapshotSummary
from qdu.patterns import PathPatternMatcher
from qdu.scanner import FilesystemScanner
from qdu.storage import (
    IndexRepository,
    ProfilePaths,
    create_snapshot_database,
    sha256_file,
)


class SnapshotRepository:
    """Create and retain snapshots for one immutable profile configuration."""

    def __init__(self, profile: ProfileConfig) -> None:
        self.profile = profile
        self.paths = ProfilePaths.for_profile(profile.name)
        self.index = IndexRepository(self.paths)

    def create(
        self, *, quiet: bool = False
    ) -> tuple[SnapshotIndexRecord, SnapshotSummary]:
        """Scan the profile root and atomically publish a new snapshot.

        The temporary database is removed after any fatal failure. Recoverable
        scan errors produce an indexed incomplete snapshot.

        Raises:
            BusyError: If another mutating profile operation holds the lock.
            UsageError: If the root is invalid or conflicts with existing history.
        """
        self.paths.ensure()
        command = f"snapshot profile={self.profile.name} root={self.profile.root}"
        with ProfileLock(self.paths.lock, command):
            existing_index = self.index.load()
            canonical_root = self.profile.root.expanduser().resolve()
            if existing_index.latest_any is not None:
                latest_record = next(
                    item
                    for item in existing_index.snapshots
                    if item.snapshot_id == existing_index.latest_any
                )
                if Path(latest_record.root) != canonical_root:
                    raise UsageError(
                        f"profile {self.profile.name!r} is already bound to {latest_record.root}; "
                        "create another profile for a different root"
                    )
            snapshot_id = _snapshot_id()
            temporary_path = self.paths.temporary / f"{snapshot_id}.sqlite3.tmp"
            final_path = self.paths.snapshots / f"{snapshot_id}.sqlite3"
            temporary_path.unlink(missing_ok=True)
            connection = create_snapshot_database(temporary_path)
            try:
                try:
                    matcher = PathPatternMatcher(self.profile.excludes)
                    scanner = FilesystemScanner(
                        connection,
                        root=self.profile.root,
                        matcher=matcher,
                        collect_users=self.profile.collect_users,
                        user_max_depth=self.profile.user_max_depth,
                        large_file_limit=self.profile.large_file_limit,
                        record_max_depth=self.profile.record_max_depth,
                        one_file_system=not self.profile.cross_filesystems,
                    )
                    summary = scanner.scan(snapshot_id)
                    self._finalize_database(connection, summary, matcher.digest)
                finally:
                    connection.close()
                os.chmod(temporary_path, 0o600)
                os.replace(temporary_path, final_path)
            except BaseException:
                temporary_path.unlink(missing_ok=True)
                raise
            digest = sha256_file(final_path)
            record = SnapshotIndexRecord(
                snapshot_id=snapshot_id,
                filename=final_path.name,
                created_epoch=summary.created_epoch,
                created_at=summary.created_at,
                root=str(summary.root),
                host=summary.host,
                complete=summary.complete,
                error_count=summary.error_count,
                allocated_bytes=summary.totals.allocated_bytes,
                apparent_bytes=summary.totals.apparent_bytes,
                directory_count=summary.totals.directory_count,
                file_count=summary.totals.file_count,
                inode_count=summary.totals.inode_count,
                collect_users=summary.collect_users,
                large_file_limit=summary.large_file_limit,
                exclude_hash=matcher.digest,
                sha256=digest,
                archived=False,
            )
            self.index.add(record)
            self.prune(self.profile.keep_snapshots)
            return record, summary

    def _finalize_database(
        self,
        connection: sqlite3.Connection,
        summary: SnapshotSummary,
        exclude_hash: str,
    ) -> None:
        metadata = {
            "snapshot_id": summary.snapshot_id,
            "created_epoch": str(summary.created_epoch),
            "created_at": summary.created_at,
            "host": summary.host,
            "root": str(summary.root),
            "complete": "true" if summary.complete else "false",
            "error_count": str(summary.error_count),
            "duration_seconds": f"{summary.duration_seconds:.6f}",
            "allocated_bytes": str(summary.totals.allocated_bytes),
            "apparent_bytes": str(summary.totals.apparent_bytes),
            "directory_count": str(summary.totals.directory_count),
            "file_count": str(summary.totals.file_count),
            "inode_count": str(summary.totals.inode_count),
            "collect_users": "true" if summary.collect_users else "false",
            "large_file_limit": str(summary.large_file_limit),
            "exclude_patterns": "\n".join(summary.exclude_patterns),
            "exclude_hash": exclude_hash,
            "filesystem_total_bytes": str(summary.filesystem_total_bytes),
            "filesystem_available_bytes": str(summary.filesystem_available_bytes),
            "filesystem_total_inodes": str(summary.filesystem_total_inodes),
            "filesystem_available_inodes": str(summary.filesystem_available_inodes),
            "cross_filesystems": "true" if summary.cross_filesystems else "false",
            "skipped_filesystem_count": str(summary.skipped_filesystem_count),
            "skipped_filesystems": "\n".join(summary.skipped_filesystems),
            "filesystem_paths": "\n".join(summary.filesystem_paths),
            "filesystem_count": str(len(summary.filesystem_paths)),
        }
        stored_directory_count = connection.execute(
            "SELECT COUNT(*) FROM directories"
        ).fetchone()[0]
        metadata["stored_directory_count"] = str(stored_directory_count)
        connection.executemany(
            "INSERT OR REPLACE INTO metadata(key, value) VALUES (?, ?)",
            metadata.items(),
        )
        connection.execute("DROP TABLE seen_inodes")
        connection.commit()
        connection.execute("PRAGMA optimize")
        connection.execute("VACUUM")

    def prune(self, keep: int) -> list[str]:
        """Remove oldest snapshots until at most ``keep`` records remain.

        A non-positive value disables retention pruning.
        """
        if keep <= 0:
            return []
        index = self.index.load()
        if len(index.snapshots) <= keep:
            return []
        removable = list(index.snapshots[: len(index.snapshots) - keep])
        removed_ids: list[str] = []
        for record in removable:
            path = self.paths.snapshots / record.filename
            path.unlink(missing_ok=True)
            removed_ids.append(record.snapshot_id)
        self.index.remove(set(removed_ids))
        return removed_ids


def _snapshot_id() -> str:
    timestamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    return f"{timestamp}-{time.time_ns() % 1_000_000_000:09d}-{uuid.uuid4().hex[:6]}"
