from __future__ import annotations

import fnmatch
import heapq
import pwd
import sqlite3
from contextlib import ExitStack, closing
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator

from qdu.errors import UsageError
from qdu.models import (
    DiffRecord,
    DirectoryRecord,
    FileRecord,
    OwnerDirectoryRecord,
    OwnerRecord,
    SnapshotIndexRecord,
)
from qdu.scanner import username_for_uid
from qdu.storage import IndexRepository, ProfilePaths, materialized_snapshot, open_snapshot_database, snapshot_metadata


@dataclass(frozen=True, slots=True)
class SnapshotContext:
    record: SnapshotIndexRecord
    path: Path
    connection: sqlite3.Connection
    metadata: dict[str, str]


class SnapshotQueryService:
    def __init__(self, profile: str) -> None:
        self.paths = ProfilePaths.for_profile(profile)
        self.index = IndexRepository(self.paths)

    def resolve(self, selector: str | None) -> SnapshotIndexRecord:
        return self.index.resolve(selector)

    def previous_of(self, record: SnapshotIndexRecord) -> SnapshotIndexRecord:
        return self.index.previous_of(record)

    def open_context(self, stack: ExitStack, record: SnapshotIndexRecord) -> SnapshotContext:
        path = stack.enter_context(materialized_snapshot(self.paths, record))
        connection = stack.enter_context(closing(open_snapshot_database(path)))
        metadata = snapshot_metadata(connection)
        return SnapshotContext(record=record, path=path, connection=connection, metadata=metadata)

    def directory_ranking(
        self,
        connection: sqlite3.Connection,
        *,
        under: str,
        max_depth: int,
        top: int,
        min_size: int,
        match: str | None,
        metric: str = "allocated_bytes",
        modified_before_epoch: float | None = None,
    ) -> tuple[DirectoryRecord | None, list[DirectoryRecord]]:
        scope = normalize_relative_path(under)
        scope_row = connection.execute(
            "SELECT path, depth, allocated_bytes, apparent_bytes, file_count, directory_count, "
            "inode_count, latest_modified_epoch FROM directories WHERE path = ?",
            (scope,),
        ).fetchone()
        if scope_row is None:
            raise UsageError(f"directory is not present in snapshot: {under}")
        scope_record = _directory_from_row(scope_row)
        max_absolute_depth = scope_record.depth + max_depth
        params: list[object] = [scope_record.depth, max_absolute_depth, min_size]
        where = ["depth > ?", "depth <= ?", f"{metric} >= ?"]
        if modified_before_epoch is not None:
            where.append("latest_modified_epoch <= ?")
            params.append(modified_before_epoch)
        if scope != ".":
            where.append("path LIKE ? ESCAPE '\\'")
            params.append(_like_prefix(scope) + "%")
        query = (
            "SELECT path, depth, allocated_bytes, apparent_bytes, file_count, directory_count, "
            "inode_count, latest_modified_epoch "
            f"FROM directories WHERE {' AND '.join(where)} ORDER BY {metric} DESC, path"
        )
        results: list[DirectoryRecord] = []
        for row in connection.execute(query, params):
            record = _directory_from_row(row)
            relative = relative_to_scope(record.path, scope)
            if match and not path_matches(relative, match):
                continue
            results.append(record)
            if len(results) >= top:
                break
        return scope_record, results

    def diff_ranking(
        self,
        previous: sqlite3.Connection,
        current: sqlite3.Connection,
        *,
        under: str,
        max_depth: int,
        top: int,
        min_size: int,
        match: str | None,
        growth_only: bool,
        shrink_only: bool,
        order: str,
    ) -> list[DiffRecord]:
        scope = normalize_relative_path(under)
        previous_scope = _scope_depth(previous, scope)
        current_scope = _scope_depth(current, scope)
        if previous_scope is None and current_scope is None:
            raise UsageError(f"directory is absent from both snapshots: {under}")
        scope_depth = current_scope if current_scope is not None else previous_scope
        assert scope_depth is not None
        previous_rows = self._directory_stream(previous, scope, scope_depth, max_depth)
        current_rows = self._directory_stream(current, scope, scope_depth, max_depth)
        heap: list[tuple[tuple[int, str], DiffRecord]] = []
        previous_row = next(previous_rows, None)
        current_row = next(current_rows, None)
        while previous_row is not None or current_row is not None:
            if current_row is None or (
                previous_row is not None and previous_row.path < current_row.path
            ):
                record = DiffRecord(previous_row.path, previous_row.allocated_bytes, 0)
                previous_row = next(previous_rows, None)
            elif previous_row is None or current_row.path < previous_row.path:
                record = DiffRecord(current_row.path, 0, current_row.allocated_bytes)
                current_row = next(current_rows, None)
            else:
                assert previous_row is not None and current_row is not None
                record = DiffRecord(
                    previous_row.path,
                    previous_row.allocated_bytes,
                    current_row.allocated_bytes,
                )
                previous_row = next(previous_rows, None)
                current_row = next(current_rows, None)
            if max(record.previous_bytes, record.current_bytes) < min_size:
                continue
            if growth_only and record.change_bytes <= 0:
                continue
            if shrink_only and record.change_bytes >= 0:
                continue
            relative = relative_to_scope(record.path, scope)
            if match and not path_matches(relative, match):
                continue
            key = diff_sort_key(record, order)
            heap_item = (key, record)
            if len(heap) < top:
                heapq.heappush(heap, heap_item)
            elif key > heap[0][0]:
                heapq.heapreplace(heap, heap_item)
        return [item[1] for item in sorted(heap, key=lambda item: item[0], reverse=True)]

    def _directory_stream(
        self,
        connection: sqlite3.Connection,
        scope: str,
        scope_depth: int,
        max_depth: int,
    ) -> Iterator[DirectoryRecord]:
        params: list[object] = [scope_depth, scope_depth + max_depth]
        where = ["depth > ?", "depth <= ?"]
        if scope != ".":
            where.append("path LIKE ? ESCAPE '\\'")
            params.append(_like_prefix(scope) + "%")
        query = (
            "SELECT path, depth, allocated_bytes, apparent_bytes, file_count, directory_count, "
            "inode_count, latest_modified_epoch "
            f"FROM directories WHERE {' AND '.join(where)} ORDER BY path"
        )
        for row in connection.execute(query, params):
            yield _directory_from_row(row)

    def owners(
        self,
        connection: sqlite3.Connection,
        *,
        top: int,
        directories_per_user: int,
        metric: str = "allocated_bytes",
    ) -> tuple[list[OwnerRecord], dict[int, list[OwnerDirectoryRecord]], int]:
        owner_rows = connection.execute(
            "SELECT uid, allocated_bytes, apparent_bytes, file_count, directory_count, inode_count "
            f"FROM owners ORDER BY {metric} DESC, uid LIMIT ?",
            (top,),
        ).fetchall()
        total_row = connection.execute(f"SELECT COALESCE(SUM({metric}), 0) FROM owners").fetchone()
        metric_total = int(total_row[0]) if total_row is not None else 0
        owners = [
            OwnerRecord(
                uid=int(row["uid"]),
                username=username_for_uid(int(row["uid"])),
                allocated_bytes=int(row["allocated_bytes"]),
                apparent_bytes=int(row["apparent_bytes"]),
                file_count=int(row["file_count"]),
                directory_count=int(row["directory_count"]),
                inode_count=int(row["inode_count"]),
            )
            for row in owner_rows
        ]
        if not owners or directories_per_user <= 0:
            return owners, {}, metric_total
        selected_uids = [owner.uid for owner in owners]
        placeholders = ",".join("?" for _ in selected_uids)
        rows = connection.execute(
            "SELECT uid, path, depth, allocated_bytes, apparent_bytes, file_count, directory_count, inode_count "
            f"FROM owner_directories WHERE uid IN ({placeholders}) AND path <> '.' "
            f"ORDER BY uid, {metric} DESC, path",
            selected_uids,
        )
        grouped: dict[int, list[OwnerDirectoryRecord]] = {uid: [] for uid in selected_uids}
        for row in rows:
            uid = int(row["uid"])
            if len(grouped[uid]) >= directories_per_user:
                continue
            grouped[uid].append(
                OwnerDirectoryRecord(
                    path=str(row["path"]),
                    depth=int(row["depth"]),
                    allocated_bytes=int(row["allocated_bytes"]),
                    apparent_bytes=int(row["apparent_bytes"]),
                    file_count=int(row["file_count"]),
                    directory_count=int(row["directory_count"]),
                    inode_count=int(row["inode_count"]),
                )
            )
        return owners, grouped, metric_total

    def large_files(
        self,
        connection: sqlite3.Connection,
        *,
        top: int,
        under: str,
        min_size: int,
        user: str | None,
        modified_before_epoch: float | None = None,
    ) -> list[FileRecord]:
        scope = normalize_relative_path(under)
        uid = resolve_user(user) if user is not None else None
        where = ["allocated_bytes >= ?"]
        params: list[object] = [min_size]
        if scope != ".":
            where.append("path LIKE ? ESCAPE '\\'")
            params.append(_like_prefix(scope) + "%")
        if uid is not None:
            where.append("uid = ?")
            params.append(uid)
        if modified_before_epoch is not None:
            where.append("modified_epoch <= ?")
            params.append(modified_before_epoch)
        params.append(top)
        rows = connection.execute(
            "SELECT path, uid, allocated_bytes, apparent_bytes, modified_epoch FROM large_files "
            f"WHERE {' AND '.join(where)} ORDER BY allocated_bytes DESC, path LIMIT ?",
            params,
        )
        return [
            FileRecord(
                path=str(row["path"]),
                uid=int(row["uid"]),
                username=username_for_uid(int(row["uid"])),
                allocated_bytes=int(row["allocated_bytes"]),
                apparent_bytes=int(row["apparent_bytes"]),
                modified_epoch=float(row["modified_epoch"]),
            )
            for row in rows
        ]


def _directory_from_row(row: sqlite3.Row) -> DirectoryRecord:
    return DirectoryRecord(
        path=str(row["path"]),
        depth=int(row["depth"]),
        allocated_bytes=int(row["allocated_bytes"]),
        apparent_bytes=int(row["apparent_bytes"]),
        file_count=int(row["file_count"]),
        directory_count=int(row["directory_count"]),
        inode_count=int(row["inode_count"]),
        latest_modified_epoch=float(row["latest_modified_epoch"]),
    )


def normalize_relative_path(value: str | None) -> str:
    if value is None or value in {"", ".", "./"}:
        return "."
    normalized = value.replace("\\", "/").strip("/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    if normalized in {"", "."}:
        return "."
    if normalized == ".." or normalized.startswith("../") or "/../" in normalized:
        raise UsageError("relative paths must stay inside the snapshot root")
    return normalized


def relative_to_scope(path: str, scope: str) -> str:
    if scope == ".":
        return path
    if path == scope:
        return "."
    prefix = scope + "/"
    return path[len(prefix) :] if path.startswith(prefix) else path


def path_matches(path: str, pattern: str) -> bool:
    if any(character in pattern for character in "*?["):
        return fnmatch.fnmatchcase(path, pattern)
    return pattern.casefold() in path.casefold()


def diff_sort_key(record: DiffRecord, order: str) -> tuple[int, str]:
    if order == "growth":
        value = record.change_bytes
    elif order == "shrink":
        value = -record.change_bytes
    elif order == "absolute":
        value = record.absolute_change_bytes
    elif order == "current":
        value = record.current_bytes
    else:
        raise UsageError(f"unsupported diff order: {order}")
    return value, record.path


def _scope_depth(connection: sqlite3.Connection, scope: str) -> int | None:
    row = connection.execute("SELECT depth FROM directories WHERE path = ?", (scope,)).fetchone()
    return int(row[0]) if row is not None else None


def _like_prefix(value: str) -> str:
    escaped = value.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")
    return escaped + "/"


def resolve_user(value: str) -> int:
    if value.isdigit():
        return int(value)
    try:
        return pwd.getpwnam(value).pw_uid
    except KeyError as exc:
        raise UsageError(f"unknown user: {value}") from exc
