"""Walk a filesystem and stream aggregate snapshot data into SQLite."""

from __future__ import annotations

import heapq
import os
import pwd
import socket
import sqlite3
import stat
import sys
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

from qdu.errors import UsageError
from qdu.models import DirectoryTotals, ScanError, SnapshotSummary
from qdu.patterns import PathPatternMatcher
from qdu.storage import publish_staged_directories

_ROW_BATCH_SIZE = 1000
_PROGRESS_CHECK_ENTRIES = 2048
# A directory keeps its listing handle open while the walk is below it. Past
# this depth, the remaining entries are read into memory first, so a very deep
# tree cannot exhaust the open-file limit (often 1,024, or 256 on macOS).
_MAX_OPEN_LISTINGS = 64
_MAX_RETAINED_ERRORS = 100
_MAX_RETAINED_SKIPPED_FILESYSTEMS = 100


@dataclass(slots=True)
class _Frame:
    relative_path: str
    depth: int
    uid: int
    own_allocated: int
    own_apparent: int
    own_modified_epoch: float
    entries: Iterator[os.DirEntry[str]]
    # Owner usage keyed by UID: [allocated, apparent, files, directories, inodes].
    # Frames at or above user_max_depth own their mapping; deeper frames share
    # the mapping of their ancestor at user_max_depth.
    owners: dict[int, list[int]] | None = None
    totals: DirectoryTotals = field(default_factory=DirectoryTotals)

    def __post_init__(self) -> None:
        self.totals.allocated_bytes = self.own_allocated
        self.totals.apparent_bytes = self.own_apparent
        self.totals.latest_modified_epoch = self.own_modified_epoch


@dataclass(frozen=True, slots=True)
class ScanProgress:
    """Counters reported periodically while a scan is running.

    ``phase`` is ``"scanning"`` during the walk and ``"writing"`` once the
    walk is done and the snapshot database is being finished.
    """

    phase: str
    entries: int
    directories: int
    allocated_bytes: int
    errors: int
    elapsed_seconds: float
    current_path: str


class FilesystemScanner:
    """Iteratively scan one root while localizing errors and inode deduplication."""

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
        progress: Callable[[ScanProgress], None] | None = None,
        progress_interval_seconds: float = 0.5,
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
        self._directory_rows: list[tuple[str, int, int, int, int, int, int, float]] = []
        self._owner_directory_rows: list[
            tuple[int, str, int, int, int, int, int, int]
        ] = []
        self._skipped_filesystem_count = 0
        self._skipped_filesystems: list[str] = []
        self._filesystem_paths: dict[int, str] = {}
        self.progress = progress
        self.progress_interval_seconds = progress_interval_seconds
        self._started_monotonic = 0.0
        self._entries_seen = 0
        self._directories_seen = 0
        self._allocated_seen = 0
        self._next_progress_check = (
            _PROGRESS_CHECK_ENTRIES if progress is not None else sys.maxsize
        )
        self._next_progress_time = 0.0

    def scan(self, snapshot_id: str) -> SnapshotSummary:
        """Populate the open snapshot database and return scan metadata.

        Raises:
            UsageError: If the root does not exist, is not a directory, or cannot
                be inspected at all.
        """
        started = time.time()
        self._started_monotonic = time.monotonic()
        root = self.root.expanduser().resolve()
        if storable_text(str(root)) != str(root):
            raise UsageError(
                f"scan root is not valid UTF-8: {storable_text(str(root))}"
            )
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
        frame = self._open_frame(
            str(root), ".", 0, root_stat, owners={} if self.collect_users else None
        )
        if frame is None:
            raise UsageError(f"cannot read scan root: {root}")

        stack: list[_Frame] = [frame]
        try:
            with self.connection:
                while stack:
                    current = stack[-1]
                    child = self._count_until_subdirectory(current, root_device)
                    if child is not None:
                        if len(stack) >= _MAX_OPEN_LISTINGS:
                            self._detach_listing(current)
                        stack.append(child)
                        continue
                    stack.pop()
                    parent = stack[-1] if stack else None
                    self._finalize_frame(current, parent)
                    if parent is not None:
                        parent.totals.add(current.totals)

                self._report_progress("writing", "")
                self._flush_directory_rows()
                publish_staged_directories(self.connection)
                self._flush_owner_directory_rows()
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

    def _count_until_subdirectory(
        self, frame: _Frame, root_device: int
    ) -> _Frame | None:
        """Count a directory's files until its listing yields a subdirectory.

        Returns:
            The frame of the subdirectory to enter next, or ``None`` once the
            listing is exhausted or broke off. The listing stays open, so the
            walk resumes here after the subdirectory is finished.
        """
        entries = frame.entries
        totals = frame.totals
        owners = frame.owners
        prefix = "" if frame.relative_path == "." else f"{frame.relative_path}/"
        is_excluded = self.matcher.matches_entry if self.matcher.patterns else None
        is_excluded_directory = (
            self.matcher.matches_directory if self.matcher.has_directory_rules else None
        )
        large_files = self._large_files
        large_file_limit = self.large_file_limit
        while True:
            try:
                entry = next(entries)
            except StopIteration:
                return None
            except OSError as exc:
                # The listing broke off, for example with ESTALE or EIO on NFS.
                # Keep the entries already counted.
                self._record_error(frame.relative_path, "readdir", exc)
                return None

            name = entry.name
            if not name.isascii():
                name = storable_text(name)
            relative_path = prefix + name
            if is_excluded is not None and is_excluded(name, relative_path):
                continue
            try:
                value = entry.stat(follow_symlinks=False)
            except OSError as exc:
                self._record_error(relative_path, "stat", exc)
                continue
            self._entries_seen += 1
            if self._entries_seen >= self._next_progress_check:
                self._next_progress_check = self._entries_seen + _PROGRESS_CHECK_ENTRIES
                self._report_progress("scanning", frame.relative_path)
            if stat.S_ISDIR(value.st_mode):
                if is_excluded_directory is not None and is_excluded_directory(
                    name, relative_path
                ):
                    continue
                child = self._enter_directory(
                    entry.path, relative_path, frame, value, root_device
                )
                if child is not None:
                    return child
                continue

            # Freshness is path-oriented rather than capacity-oriented. A
            # hard-linked file therefore updates every containing directory even
            # though its bytes and inode are counted only once in the totals.
            modified_epoch = value.st_mtime
            if modified_epoch > totals.latest_modified_epoch:
                totals.latest_modified_epoch = modified_epoch
            if value.st_nlink > 1 and not self._claim_inode(value):
                continue
            allocated = allocated_bytes(value)
            apparent = apparent_bytes(value)
            self._allocated_seen += allocated
            totals.allocated_bytes += allocated
            totals.apparent_bytes += apparent
            totals.file_count += 1
            totals.inode_count += 1
            if owners is not None:
                _add_owner_usage(
                    owners,
                    value.st_uid,
                    allocated=allocated,
                    apparent=apparent,
                    files=1,
                    directories=0,
                )
            if large_file_limit and (
                len(large_files) < large_file_limit or allocated >= large_files[0][0]
            ):
                self._consider_large_file(
                    relative_path, value.st_uid, allocated, apparent, modified_epoch
                )

    def _detach_listing(self, frame: _Frame) -> None:
        """Read the rest of a directory listing into memory and close it."""
        remaining: list[os.DirEntry[str]] = []
        while True:
            try:
                remaining.append(next(frame.entries))
            except StopIteration:
                break
            except OSError as exc:
                self._record_error(frame.relative_path, "readdir", exc)
                break
        close = getattr(frame.entries, "close", None)
        if close is not None:
            close()
        frame.entries = iter(remaining)

    def _enter_directory(
        self,
        path: str,
        relative_path: str,
        parent: _Frame,
        value: os.stat_result,
        root_device: int,
    ) -> _Frame | None:
        if value.st_dev != root_device:
            if self.one_file_system:
                self._record_skipped_filesystem(relative_path)
                return None
            self._register_filesystem(value.st_dev, relative_path)
        return self._open_frame(
            path,
            relative_path,
            parent.depth + 1,
            value,
            owners=self._child_owners(parent),
        )

    def _open_frame(
        self,
        path: str,
        relative_path: str,
        depth: int,
        path_stat: os.stat_result,
        *,
        owners: dict[int, list[int]] | None,
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
        self._directories_seen += 1
        own_allocated = allocated_bytes(path_stat)
        self._allocated_seen += own_allocated
        return _Frame(
            relative_path=relative_path,
            depth=depth,
            uid=path_stat.st_uid,
            own_allocated=own_allocated,
            own_apparent=apparent_bytes(path_stat),
            own_modified_epoch=path_stat.st_mtime,
            entries=iter(entries),
            owners=owners,
        )

    def _child_owners(self, parent: _Frame) -> dict[int, list[int]] | None:
        if parent.owners is None or parent.depth >= self.user_max_depth:
            return parent.owners
        return {}

    def _finalize_frame(self, frame: _Frame, parent: _Frame | None) -> None:
        close = getattr(frame.entries, "close", None)
        if close is not None:
            close()
        if self.record_max_depth is None or frame.depth <= self.record_max_depth:
            totals = frame.totals
            self._directory_rows.append(
                (
                    frame.relative_path,
                    frame.depth,
                    totals.allocated_bytes,
                    totals.apparent_bytes,
                    totals.file_count,
                    totals.directory_count,
                    totals.inode_count,
                    totals.latest_modified_epoch,
                )
            )
            if len(self._directory_rows) >= _ROW_BATCH_SIZE:
                self._flush_directory_rows()
        if frame.owners is not None:
            _add_owner_usage(
                frame.owners,
                frame.uid,
                allocated=frame.own_allocated,
                apparent=frame.own_apparent,
                files=0,
                directories=1,
            )
            if frame.depth <= self.user_max_depth:
                self._record_owner_directory(frame)
                if parent is None:
                    self._record_owner_totals(frame.owners)
                elif parent.owners is not None:
                    _merge_owner_usage(parent.owners, frame.owners)

    def _claim_inode(self, value: os.stat_result) -> bool:
        if value.st_nlink <= 1:
            return True
        # Directories always have st_nlink >= 2 on most filesystems; claiming
        # them as well keeps bind-mounted trees from being counted twice.
        cursor = self.connection.execute(
            "INSERT OR IGNORE INTO seen_inodes(device, inode) VALUES (?, ?)",
            (int(value.st_dev), int(value.st_ino)),
        )
        return cursor.rowcount == 1

    def _flush_directory_rows(self) -> None:
        if not self._directory_rows:
            return
        self.connection.executemany(
            "INSERT INTO temp.staged_directories(path, depth, allocated_bytes, "
            "apparent_bytes, file_count, directory_count, inode_count, "
            "latest_modified_epoch) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            self._directory_rows,
        )
        self._directory_rows.clear()

    def _record_owner_directory(self, frame: _Frame) -> None:
        if frame.owners is None:
            return
        self._owner_directory_rows.extend(
            (uid, frame.relative_path, frame.depth, *usage)
            for uid, usage in frame.owners.items()
        )
        if len(self._owner_directory_rows) >= _ROW_BATCH_SIZE:
            self._flush_owner_directory_rows()

    def _flush_owner_directory_rows(self) -> None:
        if not self._owner_directory_rows:
            return
        self.connection.executemany(
            "INSERT INTO owner_directories(uid, path, depth, allocated_bytes, apparent_bytes, "
            "file_count, directory_count, inode_count) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            self._owner_directory_rows,
        )
        self._owner_directory_rows.clear()

    def _record_owner_totals(self, owners: dict[int, list[int]]) -> None:
        self.connection.executemany(
            "INSERT INTO owners(uid, allocated_bytes, apparent_bytes, file_count, "
            "directory_count, inode_count) VALUES (?, ?, ?, ?, ?, ?)",
            ((uid, *usage) for uid, usage in owners.items()),
        )

    def _consider_large_file(
        self,
        path: str,
        uid: int,
        allocated: int,
        apparent: int,
        modified_epoch: float,
    ) -> None:
        item = (allocated, path, uid, apparent, modified_epoch)
        if len(self._large_files) < self.large_file_limit:
            heapq.heappush(self._large_files, item)
            return
        if item > self._large_files[0]:
            heapq.heapreplace(self._large_files, item)

    def _report_progress(self, phase: str, current_path: str) -> None:
        if self.progress is None:
            return
        now = time.monotonic()
        if phase == "scanning" and now < self._next_progress_time:
            return
        self._next_progress_time = now + self.progress_interval_seconds
        self.progress(
            ScanProgress(
                phase=phase,
                entries=self._entries_seen,
                directories=self._directories_seen,
                allocated_bytes=self._allocated_seen,
                errors=self._error_count,
                elapsed_seconds=now - self._started_monotonic,
                current_path=current_path,
            )
        )

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
        message = storable_text(str(error))
        self._error_count += 1
        self.connection.execute(
            "INSERT INTO scan_errors(path, operation, message) VALUES (?, ?, ?)",
            (path, operation, message),
        )
        if len(self._errors) < _MAX_RETAINED_ERRORS:
            self._errors.append(
                ScanError(path=path, operation=operation, message=message)
            )


def allocated_bytes(value: os.stat_result) -> int:
    """Return the bytes allocated to an inode; sparse holes count as zero."""
    # st_blocks counts 512-byte units on Linux and macOS, the supported systems.
    blocks = value.st_blocks
    return blocks * 512 if blocks > 0 else 0


def apparent_bytes(value: os.stat_result) -> int:
    """Return an inode's logical length, as ls -l reports it."""
    size = value.st_size
    return size if size > 0 else 0


def _add_owner_usage(
    owners: dict[int, list[int]],
    uid: int,
    *,
    allocated: int,
    apparent: int,
    files: int,
    directories: int,
) -> None:
    usage = owners.get(uid)
    if usage is None:
        owners[uid] = [allocated, apparent, files, directories, files + directories]
        return
    usage[0] += allocated
    usage[1] += apparent
    usage[2] += files
    usage[3] += directories
    usage[4] += files + directories


def _merge_owner_usage(
    target: dict[int, list[int]], source: dict[int, list[int]]
) -> None:
    for uid, (allocated, apparent, files, directories, _inodes) in source.items():
        _add_owner_usage(
            target,
            uid,
            allocated=allocated,
            apparent=apparent,
            files=files,
            directories=directories,
        )


def storable_text(value: str) -> str:
    r"""Return text that SQLite and JSON can store for a decoded file name.

    Python decodes file-name bytes that are not valid UTF-8 into lone
    surrogates, which cannot be encoded again. Those bytes are spelled as
    ``\xNN`` escapes instead; every valid name is returned unchanged.
    """
    if value.isascii():
        return value
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        try:
            raw = value.encode("utf-8", "surrogateescape")
        except UnicodeEncodeError:
            return value.encode("utf-8", "backslashreplace").decode("utf-8")
        return raw.decode("utf-8", "backslashreplace")
    return value


def username_for_uid(uid: int) -> str:
    """Resolve a UID to a local name, falling back to its decimal form."""
    try:
        return pwd.getpwuid(uid).pw_name
    except KeyError:
        return str(uid)
