from __future__ import annotations

import heapq
import os
import pwd
import socket
import sqlite3
import stat
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Iterable, Iterator

from qdu.errors import UsageError
from qdu.models import DirectoryTotals, ScanError, SnapshotSummary
from qdu.patterns import PathPatternMatcher

_OWNER_BATCH_SIZE = 4000
_MAX_RETAINED_ERRORS = 100
_MAX_RETAINED_SKIPPED_FILESYSTEMS = 100


@dataclass(slots=True)
class _Frame:
    path: Path
    relative_path: str
    depth: int
    uid: int
    own_allocated: int
    own_apparent: int
    own_modified_epoch: float
    entries: Iterator[os.DirEntry[str]]
    totals: DirectoryTotals = field(default_factory=DirectoryTotals)

    def __post_init__(self) -> None:
        self.totals.allocated_bytes = self.own_allocated
        self.totals.apparent_bytes = self.own_apparent
        self.totals.latest_modified_epoch = self.own_modified_epoch


class FilesystemScanner:
    def __init__(
        self,
        connection: sqlite3.Connection,
        *,
        root: Path,
        matcher: PathPatternMatcher,
        collect_users: bool,
        user_max_depth: int,
        large_file_limit: int,
        record_max_depth: int | None,
        one_file_system: bool = True,
    ) -> None:
        self.connection = connection
        self.root = root
        self.matcher = matcher
        self.collect_users = collect_users
        self.user_max_depth = user_max_depth
        self.large_file_limit = large_file_limit
        self.record_max_depth = record_max_depth
        self.one_file_system = one_file_system
        self._errors: list[ScanError] = []
        self._error_count = 0
        self._large_files: list[tuple[int, str, int, int, float]] = []
        self._owner_total_updates: list[tuple[int, int, int, int, int, int]] = []
        self._owner_directory_updates: list[tuple[int, str, int, int, int, int, int, int]] = []
        self._skipped_filesystem_count = 0
        self._skipped_filesystems: list[str] = []
        self._filesystem_paths: dict[int, str] = {}

    def scan(self, snapshot_id: str) -> SnapshotSummary:
        started = time.time()
        root = self.root.expanduser().resolve()
        if not root.exists():
            raise UsageError(f"scan root does not exist: {root}")
        if not root.is_dir():
            raise UsageError(f"scan root is not a directory: {root}")
        try:
            root_stat = root.stat()
        except OSError as exc:
            raise UsageError(f"cannot stat scan root {root}: {exc}") from exc

        root_device = int(root_stat.st_dev)
        self._register_filesystem(root_device, ".")
        frame = self._open_frame(root, ".", 0, root_stat)
        if frame is None:
            raise UsageError(f"cannot read scan root: {root}")

        stack: list[_Frame] = [frame]
        try:
            with self.connection:
                while stack:
                    current = stack[-1]
                    try:
                        entry = next(current.entries)
                    except StopIteration:
                        self._finalize_frame(current)
                        stack.pop()
                        if stack:
                            stack[-1].totals.add(current.totals)
                        continue

                    relative_path = entry.name if current.relative_path == "." else f"{current.relative_path}/{entry.name}"
                    if self.matcher.matches(relative_path):
                        continue
                    try:
                        entry_stat = entry.stat(follow_symlinks=False)
                    except OSError as exc:
                        self._record_error(relative_path, "stat", exc)
                        continue
                    is_directory = stat.S_ISDIR(entry_stat.st_mode) and not stat.S_ISLNK(entry_stat.st_mode)
                    if is_directory and int(entry_stat.st_dev) != root_device:
                        if self.one_file_system:
                            self._record_skipped_filesystem(relative_path)
                            continue
                        self._register_filesystem(int(entry_stat.st_dev), relative_path)

                    if is_directory:
                        child_frame = self._open_frame(
                            Path(entry.path), relative_path, current.depth + 1, entry_stat
                        )
                        if child_frame is not None:
                            stack.append(child_frame)
                        continue

                    # Freshness is path-oriented rather than capacity-oriented. A hard-linked
                    # file therefore updates every containing directory even though its bytes and
                    # inode are counted only once in the snapshot totals.
                    current.totals.latest_modified_epoch = max(
                        current.totals.latest_modified_epoch, float(entry_stat.st_mtime)
                    )
                    if not self._claim_inode(entry_stat):
                        continue
                    allocated = _allocated_bytes(entry_stat)
                    apparent = max(0, int(entry_stat.st_size))
                    current.totals.allocated_bytes += allocated
                    current.totals.apparent_bytes += apparent
                    current.totals.file_count += 1
                    current.totals.inode_count += 1
                    if self.collect_users:
                        self._queue_owner_inode(
                            uid=entry_stat.st_uid,
                            relative_container=current.relative_path,
                            allocated=allocated,
                            apparent=apparent,
                            file_count=1,
                            directory_count=0,
                        )
                    self._consider_large_file(
                        relative_path,
                        entry_stat.st_uid,
                        allocated,
                        apparent,
                        float(entry_stat.st_mtime),
                    )

                self._flush_owner_updates()
                for allocated, path, uid, apparent, modified_epoch in sorted(
                    self._large_files, key=lambda item: (-item[0], item[1])
                ):
                    self.connection.execute(
                        "INSERT INTO large_files(path, uid, allocated_bytes, apparent_bytes, modified_epoch) "
                        "VALUES (?, ?, ?, ?, ?)",
                        (path, uid, allocated, apparent, modified_epoch),
                    )
        finally:
            for open_frame in stack:
                close = getattr(open_frame.entries, "close", None)
                if close is not None:
                    close()

        ended = time.time()
        root_totals = frame.totals
        (
            filesystem_total_bytes,
            filesystem_available_bytes,
            filesystem_total_inodes,
            filesystem_available_inodes,
        ) = self._filesystem_capacity(root)
        created_epoch = int(ended)
        created_at = datetime.now().astimezone().isoformat(timespec="seconds")
        return SnapshotSummary(
            snapshot_id=snapshot_id,
            root=root,
            created_epoch=created_epoch,
            created_at=created_at,
            host=socket.gethostname(),
            complete=self._error_count == 0,
            errors=tuple(self._errors),
            error_count=self._error_count,
            duration_seconds=ended - started,
            totals=root_totals,
            collect_users=self.collect_users,
            large_file_limit=self.large_file_limit,
            exclude_patterns=self.matcher.patterns,
            filesystem_total_bytes=filesystem_total_bytes,
            filesystem_available_bytes=filesystem_available_bytes,
            filesystem_total_inodes=filesystem_total_inodes,
            filesystem_available_inodes=filesystem_available_inodes,
            cross_filesystems=not self.one_file_system,
            skipped_filesystem_count=self._skipped_filesystem_count,
            skipped_filesystems=tuple(self._skipped_filesystems),
            filesystem_paths=tuple(self._filesystem_paths.values()),
        )

    def _open_frame(
        self,
        path: Path,
        relative_path: str,
        depth: int,
        path_stat: os.stat_result,
    ) -> _Frame | None:
        try:
            entries = os.scandir(path)
        except OSError as exc:
            self._record_error(relative_path, "scandir", exc)
            return None
        if not self._claim_inode(path_stat):
            # Directories should not normally be hard-linked. Avoid double counting
            # if an unusual filesystem exposes one more than once.
            entries.close()
            return None
        return _Frame(
            path=path,
            relative_path=relative_path,
            depth=depth,
            uid=path_stat.st_uid,
            own_allocated=_allocated_bytes(path_stat),
            own_apparent=max(0, int(path_stat.st_size)),
            own_modified_epoch=float(path_stat.st_mtime),
            entries=iter(entries),
        )

    def _finalize_frame(self, frame: _Frame) -> None:
        close = getattr(frame.entries, "close", None)
        if close is not None:
            close()
        if self.record_max_depth is None or frame.depth <= self.record_max_depth:
            parent = _parent_path(frame.relative_path)
            self.connection.execute(
                "INSERT INTO directories(path, parent, depth, allocated_bytes, apparent_bytes, "
                "file_count, directory_count, inode_count, latest_modified_epoch) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    frame.relative_path,
                    parent,
                    frame.depth,
                    frame.totals.allocated_bytes,
                    frame.totals.apparent_bytes,
                    frame.totals.file_count,
                    frame.totals.directory_count,
                    frame.totals.inode_count,
                    frame.totals.latest_modified_epoch,
                ),
            )
        if self.collect_users:
            self._queue_owner_inode(
                uid=frame.uid,
                relative_container=frame.relative_path,
                allocated=frame.own_allocated,
                apparent=frame.own_apparent,
                file_count=0,
                directory_count=1,
            )
        if len(self._owner_total_updates) >= _OWNER_BATCH_SIZE:
            self._flush_owner_updates()

    def _claim_inode(self, value: os.stat_result) -> bool:
        if value.st_nlink <= 1:
            return True
        cursor = self.connection.execute(
            "INSERT OR IGNORE INTO seen_inodes(device, inode) VALUES (?, ?)",
            (int(value.st_dev), int(value.st_ino)),
        )
        return cursor.rowcount == 1

    def _queue_owner_inode(
        self,
        *,
        uid: int,
        relative_container: str,
        allocated: int,
        apparent: int,
        file_count: int,
        directory_count: int,
    ) -> None:
        inode_count = file_count + directory_count
        self._owner_total_updates.append(
            (uid, allocated, apparent, file_count, directory_count, inode_count)
        )
        for path, depth in _ancestor_directories(relative_container, self.user_max_depth):
            self._owner_directory_updates.append(
                (
                    uid,
                    path,
                    depth,
                    allocated,
                    apparent,
                    file_count,
                    directory_count,
                    inode_count,
                )
            )

    def _flush_owner_updates(self) -> None:
        if self._owner_total_updates:
            self.connection.executemany(
                "INSERT INTO owners(uid, allocated_bytes, apparent_bytes, file_count, directory_count, inode_count) "
                "VALUES (?, ?, ?, ?, ?, ?) "
                "ON CONFLICT(uid) DO UPDATE SET "
                "allocated_bytes=allocated_bytes+excluded.allocated_bytes, "
                "apparent_bytes=apparent_bytes+excluded.apparent_bytes, "
                "file_count=file_count+excluded.file_count, "
                "directory_count=directory_count+excluded.directory_count, "
                "inode_count=inode_count+excluded.inode_count",
                self._owner_total_updates,
            )
            self._owner_total_updates.clear()
        if self._owner_directory_updates:
            self.connection.executemany(
                "INSERT INTO owner_directories(uid, path, depth, allocated_bytes, apparent_bytes, "
                "file_count, directory_count, inode_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?) "
                "ON CONFLICT(uid, path) DO UPDATE SET "
                "allocated_bytes=allocated_bytes+excluded.allocated_bytes, "
                "apparent_bytes=apparent_bytes+excluded.apparent_bytes, "
                "file_count=file_count+excluded.file_count, "
                "directory_count=directory_count+excluded.directory_count, "
                "inode_count=inode_count+excluded.inode_count",
                self._owner_directory_updates,
            )
            self._owner_directory_updates.clear()

    def _consider_large_file(
        self,
        path: str,
        uid: int,
        allocated: int,
        apparent: int,
        modified_epoch: float,
    ) -> None:
        if self.large_file_limit <= 0:
            return
        item = (allocated, path, uid, apparent, modified_epoch)
        if len(self._large_files) < self.large_file_limit:
            heapq.heappush(self._large_files, item)
            return
        if item > self._large_files[0]:
            heapq.heapreplace(self._large_files, item)


    def _register_filesystem(self, device: int, relative_path: str) -> None:
        self._filesystem_paths.setdefault(device, relative_path)

    def _record_skipped_filesystem(self, relative_path: str) -> None:
        self._skipped_filesystem_count += 1
        if len(self._skipped_filesystems) < _MAX_RETAINED_SKIPPED_FILESYSTEMS:
            self._skipped_filesystems.append(relative_path)

    def _filesystem_capacity(self, root: Path) -> tuple[int, int, int, int]:
        total_bytes = 0
        available_bytes = 0
        total_inodes = 0
        available_inodes = 0
        for relative_path in self._filesystem_paths.values():
            path = root if relative_path == "." else root / relative_path
            try:
                value = os.statvfs(path)
            except OSError as exc:
                self._record_error(relative_path, "statvfs", exc)
                continue
            total_bytes += max(0, int(value.f_blocks) * int(value.f_frsize))
            available_bytes += max(0, int(value.f_bavail) * int(value.f_frsize))
            total_inodes += max(0, int(value.f_files))
            available_inodes += max(0, int(value.f_favail))
        return total_bytes, available_bytes, total_inodes, available_inodes

    def _record_error(self, path: str, operation: str, error: OSError) -> None:
        message = str(error)
        self._error_count += 1
        self.connection.execute(
            "INSERT INTO scan_errors(path, operation, message) VALUES (?, ?, ?)",
            (path, operation, message),
        )
        if len(self._errors) < _MAX_RETAINED_ERRORS:
            self._errors.append(ScanError(path=path, operation=operation, message=message))


def _allocated_bytes(value: os.stat_result) -> int:
    blocks = getattr(value, "st_blocks", None)
    if blocks is None:
        return max(0, int(value.st_size))
    return max(0, int(blocks) * 512)


def _parent_path(path: str) -> str:
    if path == "." or "/" not in path:
        return "."
    return path.rsplit("/", 1)[0]


def _ancestor_directories(path: str, max_depth: int) -> Iterable[tuple[str, int]]:
    if max_depth < 0:
        return ()
    if path == ".":
        return ((".", 0),)
    parts = path.split("/")
    limit = min(len(parts), max_depth)
    values: list[tuple[str, int]] = [(".", 0)]
    for depth in range(1, limit + 1):
        values.append(("/".join(parts[:depth]), depth))
    return tuple(values)


def username_for_uid(uid: int) -> str:
    try:
        return pwd.getpwuid(uid).pw_name
    except KeyError:
        return str(uid)
