"""Find large files by scanning the live filesystem without a snapshot."""

from __future__ import annotations

import heapq
import os
import stat
from pathlib import Path

from qdu.models import FileRecord
from qdu.patterns import PathPatternMatcher
from qdu.query import normalize_relative_path, resolve_user
from qdu.scanner import username_for_uid


def largest_files_live(
    *,
    root: Path,
    matcher: PathPatternMatcher,
    top: int,
    under: str,
    min_size: int,
    user: str | None,
    modified_before_epoch: float | None = None,
) -> tuple[list[FileRecord], list[str]]:
    """Return the largest matching live files and recoverable scan errors.

    The scan stays on the root filesystem, does not follow symlinks, and counts
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
    stack = [start]
    while stack:
        directory = stack.pop()
        try:
            entries = os.scandir(directory)
        except OSError as exc:
            errors.append(f"{directory}: {exc}")
            continue
        with entries:
            for entry in entries:
                try:
                    value = entry.stat(follow_symlinks=False)
                except OSError as exc:
                    errors.append(f"{entry.path}: {exc}")
                    continue
                is_directory = stat.S_ISDIR(value.st_mode) and not stat.S_ISLNK(
                    value.st_mode
                )
                if is_directory and value.st_dev != root_device:
                    continue
                relative = Path(entry.path).relative_to(canonical_root).as_posix()
                if matcher.matches(relative):
                    continue
                if is_directory:
                    stack.append(Path(entry.path))
                    continue
                if target_uid is not None and value.st_uid != target_uid:
                    continue
                if value.st_nlink > 1:
                    inode_key = (int(value.st_dev), int(value.st_ino))
                    if inode_key in seen_hardlinks:
                        continue
                    seen_hardlinks.add(inode_key)
                allocated = int(getattr(value, "st_blocks", 0)) * 512 or int(
                    value.st_size
                )
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
                    int(value.st_size),
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
