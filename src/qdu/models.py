"""Define immutable snapshot contracts and localized scan accumulators."""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass, field
from pathlib import Path

from qdu.profile_names import validate_profile_name


@dataclass(frozen=True, slots=True)
class ProfileConfig:
    """Validated settings for one named scan root and retention policy."""

    name: str
    root: Path
    excludes: tuple[str, ...] = ()
    keep_snapshots: int = 100
    collect_users: bool = False
    user_max_depth: int = 3
    large_file_limit: int = 1000
    record_max_depth: int | None = None
    cross_filesystems: bool = False
    capacity_limit_bytes: int | None = None
    capacity_user: str | None = None

    def __post_init__(self) -> None:
        validate_profile_name(self.name)
        if self.keep_snapshots < 0:
            raise ValueError("keep_snapshots must be non-negative")
        if self.user_max_depth < 0:
            raise ValueError("user_max_depth must be non-negative")
        if self.large_file_limit < 0:
            raise ValueError("large_file_limit must be non-negative")
        if self.record_max_depth is not None and self.record_max_depth < 0:
            raise ValueError("record_max_depth must be non-negative")
        if self.capacity_limit_bytes is not None and self.capacity_limit_bytes <= 0:
            raise ValueError("capacity_limit_bytes must be greater than zero")
        if self.capacity_user is not None and not self.capacity_user.strip():
            raise ValueError("capacity_user must not be empty")
        if self.capacity_user is not None and self.capacity_limit_bytes is None:
            raise ValueError("capacity_user requires capacity_limit_bytes")
        if self.capacity_user is not None and not self.collect_users:
            raise ValueError("capacity_user requires collect_users")


@dataclass(frozen=True, slots=True)
class SnapshotIndexRecord:
    """Trusted index metadata for one stored snapshot file."""

    snapshot_id: str
    filename: str
    created_epoch: int
    created_at: str
    root: str
    host: str
    complete: bool
    error_count: int
    allocated_bytes: int
    apparent_bytes: int
    directory_count: int
    file_count: int
    inode_count: int
    collect_users: bool
    large_file_limit: int
    exclude_hash: str
    sha256: str
    archived: bool = False

    def __post_init__(self) -> None:
        if (
            not self.filename
            or "/" in self.filename
            or "\\" in self.filename
            or not self.filename.endswith((".sqlite3", ".sqlite3.gz"))
        ):
            raise ValueError(
                "snapshot filename must be a safe SQLite snapshot basename"
            )

    @classmethod
    def from_dict(cls, value: dict[str, object]) -> SnapshotIndexRecord:
        """Build and validate an index record decoded from JSON.

        Raises:
            KeyError: If a required field is absent.
            TypeError: If a field cannot be converted to its required shape.
            ValueError: If the snapshot filename is unsafe.
        """
        return cls(
            snapshot_id=str(value["snapshot_id"]),
            filename=str(value["filename"]),
            created_epoch=int(value["created_epoch"]),
            created_at=str(value["created_at"]),
            root=str(value["root"]),
            host=str(value["host"]),
            complete=bool(value["complete"]),
            error_count=int(value["error_count"]),
            allocated_bytes=int(value["allocated_bytes"]),
            apparent_bytes=int(value["apparent_bytes"]),
            directory_count=int(value["directory_count"]),
            file_count=int(value["file_count"]),
            inode_count=int(value["inode_count"]),
            collect_users=bool(value["collect_users"]),
            large_file_limit=int(value["large_file_limit"]),
            exclude_hash=str(value["exclude_hash"]),
            sha256=str(value["sha256"]),
            archived=bool(value.get("archived", False)),
        )

    def to_dict(self) -> dict[str, object]:
        """Return the stable JSON representation stored in the profile index."""
        return {
            "snapshot_id": self.snapshot_id,
            "filename": self.filename,
            "created_epoch": self.created_epoch,
            "created_at": self.created_at,
            "root": self.root,
            "host": self.host,
            "complete": self.complete,
            "error_count": self.error_count,
            "allocated_bytes": self.allocated_bytes,
            "apparent_bytes": self.apparent_bytes,
            "directory_count": self.directory_count,
            "file_count": self.file_count,
            "inode_count": self.inode_count,
            "collect_users": self.collect_users,
            "large_file_limit": self.large_file_limit,
            "exclude_hash": self.exclude_hash,
            "sha256": self.sha256,
            "archived": self.archived,
        }


@dataclass(frozen=True, slots=True)
class DirectoryRecord:
    """Aggregated usage and freshness for one snapshot directory."""

    path: str
    depth: int
    allocated_bytes: int
    apparent_bytes: int
    file_count: int
    directory_count: int
    inode_count: int
    latest_modified_epoch: float


@dataclass(frozen=True, slots=True)
class OwnerDirectoryRecord:
    """Usage attributed to one owner below a recorded directory."""

    path: str
    depth: int
    allocated_bytes: int
    apparent_bytes: int
    file_count: int
    directory_count: int
    inode_count: int


@dataclass(frozen=True, slots=True)
class OwnerRecord:
    """Snapshot-wide usage attributed to one filesystem owner."""

    uid: int
    username: str
    allocated_bytes: int
    apparent_bytes: int
    file_count: int
    directory_count: int
    inode_count: int


@dataclass(frozen=True, slots=True)
class FileRecord:
    """Size, owner, and modification metadata for one large file candidate."""

    path: str
    uid: int
    username: str
    allocated_bytes: int
    apparent_bytes: int
    modified_epoch: float


@dataclass(frozen=True, slots=True)
class ScanError:
    """A recoverable filesystem operation failure captured during scanning."""

    path: str
    operation: str
    message: str


@dataclass(slots=True)
class DirectoryTotals:
    """Mutable accumulator used while walking a directory subtree."""

    allocated_bytes: int = 0
    apparent_bytes: int = 0
    file_count: int = 0
    directory_count: int = 1
    inode_count: int = 1
    latest_modified_epoch: float = 0.0

    def add(self, other: DirectoryTotals) -> None:
        """Merge a child subtree into this accumulator."""
        self.allocated_bytes += other.allocated_bytes
        self.apparent_bytes += other.apparent_bytes
        self.file_count += other.file_count
        self.directory_count += other.directory_count
        self.inode_count += other.inode_count
        self.latest_modified_epoch = max(
            self.latest_modified_epoch, other.latest_modified_epoch
        )


@dataclass(frozen=True, slots=True)
class SnapshotSummary:
    """Complete scan outcome used to finalize snapshot metadata."""

    snapshot_id: str
    root: Path
    created_epoch: int
    created_at: str
    host: str
    complete: bool
    errors: tuple[ScanError, ...]
    error_count: int
    duration_seconds: float
    totals: DirectoryTotals
    collect_users: bool
    large_file_limit: int
    exclude_patterns: tuple[str, ...]
    filesystem_total_bytes: int
    filesystem_available_bytes: int
    filesystem_total_inodes: int
    filesystem_available_inodes: int
    cross_filesystems: bool
    skipped_filesystem_count: int
    skipped_filesystems: tuple[str, ...]
    filesystem_paths: tuple[str, ...]


@dataclass(frozen=True, slots=True)
class DiffRecord:
    """Allocated bytes for one path at two snapshot points."""

    path: str
    previous_bytes: int
    current_bytes: int

    @property
    def change_bytes(self) -> int:
        """Signed current-minus-previous byte change."""
        return self.current_bytes - self.previous_bytes

    @property
    def absolute_change_bytes(self) -> int:
        """Magnitude of the byte change regardless of direction."""
        return abs(self.change_bytes)


@dataclass(frozen=True, slots=True)
class CheckResult:
    """Observed outcome for one operational threshold check."""

    name: str
    passed: bool
    observed: str
    threshold: str
    detail: str = ""


@dataclass(frozen=True, slots=True)
class DoctorItem:
    """Status and diagnostic detail for one environment check."""

    name: str
    status: str
    detail: str


@dataclass(frozen=True, slots=True)
class ProfileIndex:
    """Ordered snapshot history and latest selectors for one profile."""

    version: int
    profile: str
    latest_complete: str | None
    latest_any: str | None
    snapshots: tuple[SnapshotIndexRecord, ...] = field(default_factory=tuple)

    def to_dict(self) -> dict[str, object]:
        """Return the stable JSON representation stored on disk."""
        return {
            "version": self.version,
            "profile": self.profile,
            "latest_complete": self.latest_complete,
            "latest_any": self.latest_any,
            "snapshots": [record.to_dict() for record in self.snapshots],
        }

    @classmethod
    def from_dict(cls, value: dict[str, object]) -> ProfileIndex:
        """Decode a profile index while validating its snapshot collection."""
        raw_snapshots = value.get("snapshots", [])
        if not isinstance(raw_snapshots, list):
            raise ValueError("snapshots must be a list")
        return cls(
            version=int(value["version"]),
            profile=str(value["profile"]),
            latest_complete=(
                str(value["latest_complete"])
                if value.get("latest_complete") is not None
                else None
            ),
            latest_any=(
                str(value["latest_any"])
                if value.get("latest_any") is not None
                else None
            ),
            snapshots=tuple(
                SnapshotIndexRecord.from_dict(record)
                for record in raw_snapshots
                if isinstance(record, dict)
            ),
        )

    def with_records(self, records: Iterable[SnapshotIndexRecord]) -> ProfileIndex:
        """Return a chronologically ordered index with latest selectors rebuilt."""
        ordered = tuple(sorted(records, key=lambda item: item.created_epoch))
        latest_any = ordered[-1].snapshot_id if ordered else None
        complete = [record for record in ordered if record.complete]
        latest_complete = complete[-1].snapshot_id if complete else None
        return ProfileIndex(
            version=self.version,
            profile=self.profile,
            latest_complete=latest_complete,
            latest_any=latest_any,
            snapshots=ordered,
        )
