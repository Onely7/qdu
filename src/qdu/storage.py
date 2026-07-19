from __future__ import annotations

import contextlib
import gzip
import hashlib
import json
import os
import shutil
import sqlite3
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Iterator, Sequence

from qdu.config import xdg_state_home
from qdu.errors import SnapshotFormatError, UsageError, VerificationError
from qdu.models import ProfileIndex, SnapshotIndexRecord

INDEX_VERSION = 1
SNAPSHOT_SCHEMA_VERSION = 2


SCHEMA_SQL = """
PRAGMA foreign_keys = ON;
CREATE TABLE metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE directories (
    path TEXT PRIMARY KEY,
    parent TEXT NOT NULL,
    depth INTEGER NOT NULL CHECK(depth >= 0),
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    directory_count INTEGER NOT NULL CHECK(directory_count >= 1),
    inode_count INTEGER NOT NULL CHECK(inode_count >= 1),
    latest_modified_epoch REAL NOT NULL CHECK(latest_modified_epoch >= 0)
) WITHOUT ROWID;
CREATE INDEX directories_allocated_idx ON directories(allocated_bytes DESC, path);
CREATE INDEX directories_inode_idx ON directories(inode_count DESC, path);
CREATE INDEX directories_modified_idx ON directories(latest_modified_epoch, path);
CREATE TABLE owners (
    uid INTEGER PRIMARY KEY,
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    directory_count INTEGER NOT NULL CHECK(directory_count >= 0),
    inode_count INTEGER NOT NULL CHECK(inode_count >= 0)
) WITHOUT ROWID;
CREATE INDEX owners_allocated_idx ON owners(allocated_bytes DESC, uid);
CREATE TABLE owner_directories (
    uid INTEGER NOT NULL,
    path TEXT NOT NULL,
    depth INTEGER NOT NULL CHECK(depth >= 0),
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    file_count INTEGER NOT NULL CHECK(file_count >= 0),
    directory_count INTEGER NOT NULL CHECK(directory_count >= 0),
    inode_count INTEGER NOT NULL CHECK(inode_count >= 0),
    PRIMARY KEY(uid, path)
) WITHOUT ROWID;
CREATE INDEX owner_directories_rank_idx
    ON owner_directories(uid, allocated_bytes DESC, path);
CREATE TABLE large_files (
    path TEXT PRIMARY KEY,
    uid INTEGER NOT NULL,
    allocated_bytes INTEGER NOT NULL CHECK(allocated_bytes >= 0),
    apparent_bytes INTEGER NOT NULL CHECK(apparent_bytes >= 0),
    modified_epoch REAL NOT NULL
) WITHOUT ROWID;
CREATE INDEX large_files_allocated_idx ON large_files(allocated_bytes DESC, path);
CREATE TABLE scan_errors (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    operation TEXT NOT NULL,
    message TEXT NOT NULL
);
CREATE TABLE seen_inodes (
    device INTEGER NOT NULL,
    inode INTEGER NOT NULL,
    PRIMARY KEY(device, inode)
) WITHOUT ROWID;
"""


@dataclass(frozen=True, slots=True)
class ProfilePaths:
    profile: str
    root: Path
    snapshots: Path
    index: Path
    lock: Path
    temporary: Path

    @classmethod
    def for_profile(cls, profile: str) -> ProfilePaths:
        root = xdg_state_home() / "qdu" / "profiles" / profile
        return cls(
            profile=profile,
            root=root,
            snapshots=root / "snapshots",
            index=root / "index.json",
            lock=root / ".lock",
            temporary=root / "tmp",
        )

    def ensure(self) -> None:
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.snapshots.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.temporary.mkdir(parents=True, exist_ok=True, mode=0o700)


class IndexRepository:
    def __init__(self, paths: ProfilePaths) -> None:
        self.paths = paths

    def load(self) -> ProfileIndex:
        if not self.paths.index.exists():
            return ProfileIndex(
                version=INDEX_VERSION,
                profile=self.paths.profile,
                latest_complete=None,
                latest_any=None,
                snapshots=(),
            )
        try:
            with self.paths.index.open("r", encoding="utf-8") as handle:
                raw = json.load(handle)
            if not isinstance(raw, dict):
                raise ValueError("index root must be an object")
            index = ProfileIndex.from_dict(raw)
        except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
            raise SnapshotFormatError(f"invalid profile index: {self.paths.index}: {exc}") from exc
        if index.version != INDEX_VERSION:
            raise SnapshotFormatError(
                f"unsupported profile index version {index.version}; expected {INDEX_VERSION}"
            )
        if index.profile != self.paths.profile:
            raise SnapshotFormatError("profile index belongs to a different profile")
        return index

    def save(self, index: ProfileIndex) -> None:
        self.paths.ensure()
        temporary = self.paths.index.with_suffix(".tmp")
        with temporary.open("w", encoding="utf-8") as handle:
            json.dump(index.to_dict(), handle, ensure_ascii=False, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, 0o600)
        os.replace(temporary, self.paths.index)
        _fsync_directory(self.paths.root)

    def add(self, record: SnapshotIndexRecord) -> ProfileIndex:
        current = self.load()
        records = [item for item in current.snapshots if item.snapshot_id != record.snapshot_id]
        records.append(record)
        updated = current.with_records(records)
        self.save(updated)
        return updated

    def remove(self, snapshot_ids: set[str]) -> ProfileIndex:
        current = self.load()
        updated = current.with_records(
            item for item in current.snapshots if item.snapshot_id not in snapshot_ids
        )
        self.save(updated)
        return updated

    def resolve(self, selector: str | None, *, include_incomplete: bool = False) -> SnapshotIndexRecord:
        index = self.load()
        if not index.snapshots:
            raise UsageError("no snapshots are available; run 'qdu snapshot' first")
        normalized = selector or "latest"
        if normalized == "latest":
            snapshot_id = index.latest_complete
            if snapshot_id is None:
                raise UsageError("no complete snapshot is available; use --snapshot latest-any")
            return _record_by_id(index.snapshots, snapshot_id)
        if normalized == "latest-any":
            if index.latest_any is None:
                raise UsageError("no snapshot is available")
            return _record_by_id(index.snapshots, index.latest_any)
        if normalized == "previous":
            latest_id = index.latest_complete if not include_incomplete else index.latest_any
            if latest_id is None:
                raise UsageError("no suitable latest snapshot is available")
            latest_position = next(
                position for position, item in enumerate(index.snapshots) if item.snapshot_id == latest_id
            )
            candidates = [
                item
                for item in index.snapshots[:latest_position]
                if include_incomplete or item.complete
            ]
            if not candidates:
                raise UsageError("no previous snapshot is available")
            return candidates[-1]
        exact = [item for item in index.snapshots if item.snapshot_id == normalized or item.filename == normalized]
        if exact:
            return exact[0]
        prefix = [item for item in index.snapshots if item.snapshot_id.startswith(normalized)]
        if len(prefix) == 1:
            return prefix[0]
        if len(prefix) > 1:
            raise UsageError(f"snapshot selector is ambiguous: {normalized}")
        raise UsageError(f"snapshot not found: {normalized}")

    def previous_of(self, record: SnapshotIndexRecord, *, complete_only: bool = True) -> SnapshotIndexRecord:
        index = self.load()
        before = []
        for item in index.snapshots:
            if item.snapshot_id == record.snapshot_id:
                break
            if not complete_only or item.complete:
                before.append(item)
        if not before:
            raise UsageError(f"no previous snapshot exists before {record.snapshot_id}")
        return before[-1]


def _record_by_id(records: Sequence[SnapshotIndexRecord], snapshot_id: str) -> SnapshotIndexRecord:
    for record in records:
        if record.snapshot_id == snapshot_id:
            return record
    raise SnapshotFormatError(f"index references a missing snapshot: {snapshot_id}")


def create_snapshot_database(path: Path) -> sqlite3.Connection:
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    connection = sqlite3.connect(path)
    connection.execute("PRAGMA journal_mode = OFF")
    connection.execute("PRAGMA synchronous = OFF")
    connection.execute("PRAGMA temp_store = FILE")
    connection.executescript(SCHEMA_SQL)
    connection.execute(
        "INSERT INTO metadata(key, value) VALUES (?, ?)",
        ("schema_version", str(SNAPSHOT_SCHEMA_VERSION)),
    )
    return connection


def open_snapshot_database(path: Path, *, read_only: bool = True) -> sqlite3.Connection:
    mode = "ro" if read_only else "rw"
    connection = sqlite3.connect(f"file:{path}?mode={mode}", uri=True)
    connection.row_factory = sqlite3.Row
    if read_only:
        connection.execute("PRAGMA query_only = ON")
    return connection


@contextlib.contextmanager
def materialized_snapshot(
    paths: ProfilePaths,
    record: SnapshotIndexRecord,
) -> Iterator[Path]:
    source = paths.snapshots / record.filename
    if not source.exists():
        raise SnapshotFormatError(f"snapshot file is missing: {source}")
    if not record.archived:
        yield source
        return
    paths.temporary.mkdir(parents=True, exist_ok=True, mode=0o700)
    file_descriptor, temporary_name = tempfile.mkstemp(
        prefix=f"{record.snapshot_id}.", suffix=".sqlite3", dir=paths.temporary
    )
    os.close(file_descriptor)
    temporary_path = Path(temporary_name)
    try:
        with gzip.open(source, "rb") as compressed, temporary_path.open("wb") as target:
            shutil.copyfileobj(compressed, target, length=1024 * 1024)
        yield temporary_path
    finally:
        temporary_path.unlink(missing_ok=True)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def snapshot_metadata(connection: sqlite3.Connection) -> dict[str, str]:
    return {str(row[0]): str(row[1]) for row in connection.execute("SELECT key, value FROM metadata")}


def validate_database(path: Path) -> dict[str, str]:
    try:
        with contextlib.closing(open_snapshot_database(path)) as connection:
            integrity = connection.execute("PRAGMA integrity_check").fetchone()
            if integrity is None or str(integrity[0]).lower() != "ok":
                raise VerificationError(f"SQLite integrity check failed: {integrity[0] if integrity else 'no result'}")
            metadata = snapshot_metadata(connection)
            version = int(metadata.get("schema_version", "0"))
            if version != SNAPSHOT_SCHEMA_VERSION:
                raise VerificationError(
                    f"unsupported snapshot schema {version}; expected {SNAPSHOT_SCHEMA_VERSION}"
                )
            directory_count = connection.execute("SELECT COUNT(*) FROM directories").fetchone()[0]
            if int(metadata.get("stored_directory_count", "-1")) != directory_count:
                raise VerificationError(
                    f"directory count mismatch: metadata={metadata.get('stored_directory_count')} actual={directory_count}"
                )
            foreign_keys = connection.execute("PRAGMA foreign_key_check").fetchall()
            if foreign_keys:
                raise VerificationError(f"foreign-key violations: {len(foreign_keys)}")
            return metadata
    except sqlite3.DatabaseError as exc:
        raise VerificationError(f"cannot read snapshot database: {exc}") from exc


def gzip_snapshot(source: Path, destination: Path) -> None:
    temporary = destination.with_suffix(destination.suffix + ".tmp")
    with source.open("rb") as input_handle, gzip.open(temporary, "wb", compresslevel=6) as output_handle:
        shutil.copyfileobj(input_handle, output_handle, length=1024 * 1024)
    os.replace(temporary, destination)
    _fsync_directory(destination.parent)


def _fsync_directory(path: Path) -> None:
    try:
        descriptor = os.open(path, os.O_DIRECTORY)
    except (AttributeError, OSError):
        return
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
