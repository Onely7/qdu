"""Find large files by scanning the live filesystem without a snapshot."""

from __future__ import annotations

import heapq
import os
import stat
from pathlib import Path

from qdu.models import FileRecord
from qdu.patterns import PathPatternMatcher
from qdu.query import normalize_relative_path, resolve_user
from qdu.scanner import (
    allocated_bytes,
    apparent_bytes,
    storable_text,
    username_for_uid,
)


def largest_files_live(
    *,
    root: Path,
    matcher: PathPatternMatcher,
    top: int,
    under: str,
    min_size: int,
    user: str | None,
    modified_before_epoch: float | None = None,
    one_file_system: bool = True,
) -> tuple[list[FileRecord], list[str]]:
    """Return the largest matching live files and recoverable scan errors.

    Like a snapshot, the scan stays on the root filesystem unless
    ``one_file_system`` is false. It does not follow symlinks and counts
    hard-linked inodes once. Errors are returned instead of aborting the scan.
    """
    canonical_root = root.expanduser().resolve()
    scope = normalize_relative_path(under)
    start = canonical_root if scope == "." else canonical_root / scope
    target_uid = resolve_user(user) if user is not None else None
    errors: list[str] = []
    heap: list[tuple[int, str, int, int, float]] = []
    seen_hardlinks: set[tuple[int, int]] = set()
    try:
        root_device = canonical_root.stat().st_dev
    except OSError as exc:
        return [], [f"{canonical_root}: {exc}"]
    if matcher.matches(scope, is_directory=True):
        # The walk starts inside the scope, so its own components are only
        # checked here; an excluded scope contributes no files.
        return [], []
    # Each stack item pairs the real directory path with its displayable
    # root-relative spelling.
    stack: list[tuple[str, str]] = [(str(start), scope)]
    while stack:
        directory, relative_directory = stack.pop()
        try:
            entries = os.scandir(directory)
        except OSError as exc:
            errors.append(storable_text(f"{directory}: {exc}"))
            continue
        with entries:
            while True:
                try:
                    entry = next(entries)
                except StopIteration:
                    break
                except OSError as exc:
                    errors.append(storable_text(f"{directory}: {exc}"))
                    break
                name = storable_text(entry.name)
                relative = (
                    name
                    if relative_directory == "."
                    else f"{relative_directory}/{name}"
                )
                if matcher.matches_entry(name, relative):
                    continue
                try:
                    value = entry.stat(follow_symlinks=False)
                except OSError as exc:
                    errors.append(storable_text(f"{entry.path}: {exc}"))
                    continue
                if stat.S_ISDIR(value.st_mode):
                    if (
                        one_file_system and value.st_dev != root_device
                    ) or matcher.matches_directory(name, relative):
                        continue
                    stack.append((entry.path, relative))
                    continue
                if target_uid is not None and value.st_uid != target_uid:
                    continue
                if value.st_nlink > 1:
                    inode_key = (int(value.st_dev), int(value.st_ino))
                    if inode_key in seen_hardlinks:
                        continue
                    seen_hardlinks.add(inode_key)
                allocated = allocated_bytes(value)
                if allocated < min_size:
                    continue
                if (
                    modified_before_epoch is not None
                    and float(value.st_mtime) > modified_before_epoch
                ):
                    continue
                item = (
                    allocated,
                    relative,
                    int(value.st_uid),
                    apparent_bytes(value),
                    float(value.st_mtime),
                )
                if len(heap) < top:
                    heapq.heappush(heap, item)
                elif item > heap[0]:
                    heapq.heapreplace(heap, item)
    records = [
        FileRecord(
            path=path,
            uid=uid,
            username=username_for_uid(uid),
            allocated_bytes=allocated,
            apparent_bytes=apparent,
            modified_epoch=modified,
        )
        for allocated, path, uid, apparent, modified in sorted(heap, reverse=True)
    ]
    return records, errors
