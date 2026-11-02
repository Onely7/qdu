from __future__ import annotations

import os
import sqlite3
import tempfile
import unittest
from contextlib import closing
from pathlib import Path
from unittest.mock import patch

from qdu.live import largest_files_live
from qdu.patterns import PathPatternMatcher
from qdu.scanner import FilesystemScanner, ScanProgress
from qdu.storage import (
    create_snapshot_database,
)

_STAT_TUPLE_INDEX = {"st_dev": 2, "st_uid": 4}
_STAT_HIDDEN_FIELDS = ("st_atime", "st_mtime", "st_ctime", "st_blksize", "st_blocks")


def _replace_stat(value: os.stat_result, **changes: int) -> os.stat_result:
    """Copy a stat result, including fields outside its tuple, with changes."""
    fields = list(value)
    for name, replacement in changes.items():
        fields[_STAT_TUPLE_INDEX[name]] = replacement
    hidden = {name: getattr(value, name) for name in _STAT_HIDDEN_FIELDS}
    return os.stat_result(fields, hidden)


class FilesystemBoundaryTest(unittest.TestCase):
    def test_cross_filesystems_controls_mount_descent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            root = base / "root"
            mounted = root / "mounted"
            mounted.mkdir(parents=True)
            (mounted / "data.bin").write_bytes(b"x" * 1024)
            root_device = root.stat().st_dev
            fake_device = root_device + 1000
            real_scandir = os.scandir

            class EntryWrapper:
                def __init__(self, entry):
                    self._entry = entry
                    self.name = entry.name
                    self.path = entry.path

                def stat(self, *, follow_symlinks=True):
                    value = self._entry.stat(follow_symlinks=follow_symlinks)
                    return _replace_stat(value, st_dev=fake_device)

            class ScandirWrapper:
                def __init__(self, entries):
                    self._iterator = iter(entries)

                def __iter__(self):
                    return self

                def __next__(self):
                    return next(self._iterator)

                def close(self):
                    return None

                def __enter__(self):
                    return self

                def __exit__(self, *exc_info):
                    return None

            def fake_scandir(path):
                path = Path(path)
                if path == root:
                    with real_scandir(path) as entries:
                        wrapped = [
                            EntryWrapper(entry) if entry.name == "mounted" else entry
                            for entry in entries
                        ]
                    return ScandirWrapper(wrapped)
                return real_scandir(path)

            summaries = []
            for one_file_system in (True, False):
                database = base / f"{one_file_system}.sqlite3"
                connection = create_snapshot_database(database)
                try:
                    scanner = FilesystemScanner(
                        connection,
                        root=root,
                        matcher=PathPatternMatcher(()),
                        collect_users=False,
                        user_max_depth=0,
                        large_file_limit=0,
                        record_max_depth=None,
                        one_file_system=one_file_system,
                    )
                    with patch("qdu.scanner.os.scandir", side_effect=fake_scandir):
                        summaries.append(scanner.scan(f"snapshot-{one_file_system}"))
                finally:
                    connection.close()

            restricted, crossing = summaries
            self.assertEqual(restricted.totals.file_count, 0)
            self.assertEqual(restricted.skipped_filesystem_count, 1)
            self.assertEqual(restricted.skipped_filesystems, ("mounted",))
            self.assertFalse(restricted.cross_filesystems)
            self.assertEqual(crossing.totals.file_count, 1)
            self.assertEqual(crossing.skipped_filesystem_count, 0)
            self.assertTrue(crossing.cross_filesystems)

            # 'qdu files --live' follows the same boundary as the profile.
            for one_file_system, expected in (
                (True, []),
                (False, ["mounted/data.bin"]),
            ):
                with patch("qdu.live.os.scandir", side_effect=fake_scandir):
                    records, errors = largest_files_live(
                        root=root,
                        matcher=PathPatternMatcher(()),
                        top=5,
                        under=".",
                        min_size=0,
                        user=None,
                        one_file_system=one_file_system,
                    )
                self.assertEqual(errors, [])
                self.assertEqual([record.path for record in records], expected)


class OwnerAggregationTest(unittest.TestCase):
    def test_owner_rows_sum_each_uid_up_to_user_max_depth(self) -> None:
        user_max_depth = 2
        other_uid = os.getuid() + 4242
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            root = base / "root"
            for relative, size in {
                "a/mine": 1000,
                "a/other-1": 3000,
                "a/b/c/d/other-2": 5000,
                "e/mine": 7000,
            }.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b"x" * size)
            real_scandir = os.scandir

            class OwnedEntry:
                """Report files named other-* as owned by another user."""

                def __init__(self, entry):
                    self._entry = entry
                    self.name = entry.name
                    self.path = entry.path

                def stat(self, *, follow_symlinks=True):
                    value = self._entry.stat(follow_symlinks=follow_symlinks)
                    if self.name.startswith("other-"):
                        return _replace_stat(value, st_uid=other_uid)
                    return value

            def fake_scandir(path):
                with real_scandir(path) as entries:
                    return iter([OwnedEntry(entry) for entry in entries])

            # Expected values follow the documented rule: every inode counts
            # toward its containing directory's ancestors up to user_max_depth.
            expected: dict[tuple[int, str], list[int]] = {}
            for current, _directories, files in os.walk(root):
                container = Path(current).relative_to(root).as_posix()
                entries = [(container, Path(current), True)]
                entries += [(container, Path(current) / name, False) for name in files]
                for parent, path, is_directory in entries:
                    value = path.lstat()
                    uid = other_uid if path.name.startswith("other-") else value.st_uid
                    owner_path = (
                        path.relative_to(root).as_posix() if is_directory else parent
                    )
                    parts = [] if owner_path == "." else owner_path.split("/")
                    for depth in range(min(len(parts), user_max_depth) + 1):
                        key = (uid, "/".join(parts[:depth]) or ".")
                        usage = expected.setdefault(key, [0, 0, 0, 0, 0])
                        usage[0] += value.st_blocks * 512
                        usage[1] += value.st_size
                        usage[2] += 0 if is_directory else 1
                        usage[3] += 1 if is_directory else 0
                        usage[4] += 1

            database = base / "snapshot.sqlite3"
            connection = create_snapshot_database(database)
            try:
                scanner = FilesystemScanner(
                    connection,
                    root=root,
                    matcher=PathPatternMatcher(()),
                    collect_users=True,
                    user_max_depth=user_max_depth,
                    large_file_limit=0,
                    record_max_depth=None,
                )
                with patch("qdu.scanner.os.scandir", side_effect=fake_scandir):
                    scanner.scan("owners")
            finally:
                connection.close()

            with closing(sqlite3.connect(database)) as reader:
                stored = {
                    (uid, path): list(usage)
                    for uid, path, *usage in reader.execute(
                        "SELECT uid, path, allocated_bytes, apparent_bytes, file_count, "
                        "directory_count, inode_count FROM owner_directories"
                    )
                }
                totals = {
                    uid: list(usage)
                    for uid, *usage in reader.execute(
                        "SELECT uid, allocated_bytes, apparent_bytes, file_count, "
                        "directory_count, inode_count FROM owners"
                    )
                }
            self.assertEqual(stored, expected)
            self.assertIn((other_uid, "a/b"), stored)
            self.assertNotIn((other_uid, "a/b/c"), stored)
            self.assertEqual(
                totals,
                {uid: usage for (uid, path), usage in expected.items() if path == "."},
            )


class ScanProgressTest(unittest.TestCase):
    def test_progress_reports_growing_counters_then_the_writing_phase(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            root = base / "root"
            for index in range(5):
                path = root / f"dir-{index}" / "file"
                path.parent.mkdir(parents=True)
                path.write_bytes(b"x" * 4096)
            events: list[ScanProgress] = []
            connection = create_snapshot_database(base / "snapshot.sqlite3")
            try:
                with patch("qdu.scanner._PROGRESS_CHECK_ENTRIES", 1):
                    scanner = FilesystemScanner(
                        connection,
                        root=root,
                        matcher=PathPatternMatcher(()),
                        collect_users=False,
                        user_max_depth=0,
                        large_file_limit=0,
                        record_max_depth=None,
                        progress=events.append,
                        progress_interval_seconds=0,
                    )
                    summary = scanner.scan("progress")
            finally:
                connection.close()

        scanning = [event for event in events if event.phase == "scanning"]
        self.assertTrue(scanning)
        counts = [event.entries for event in scanning]
        self.assertEqual(counts, sorted(set(counts)))
        final = events[-1]
        self.assertEqual(final.phase, "writing")
        self.assertEqual(final.entries, 10)
        self.assertEqual(final.directories, summary.totals.directory_count)
        self.assertEqual(final.allocated_bytes, summary.totals.allocated_bytes)


class DeepTreeTest(unittest.TestCase):
    def test_deep_trees_keep_a_bounded_number_of_listings_open(self) -> None:
        depth = 40
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory).resolve()
            root = base / "root"
            leaf = root.joinpath(*(["d"] * depth))
            leaf.mkdir(parents=True)
            (leaf / "file").write_bytes(b"x" * 4096)
            (root / "d" / "sibling").write_bytes(b"x" * 4096)
            real_scandir = os.scandir
            open_listings = [0, 0]  # current, peak

            class CountedListing:
                def __init__(self, path):
                    self._inner = real_scandir(path)
                    open_listings[0] += 1
                    open_listings[1] = max(open_listings[1], open_listings[0])

                def __iter__(self):
                    return self

                def __next__(self):
                    return next(self._inner)

                def close(self):
                    if self._inner is not None:
                        self._inner.close()
                        self._inner = None
                        open_listings[0] -= 1

            connection = create_snapshot_database(base / "snapshot.sqlite3")
            try:
                scanner = FilesystemScanner(
                    connection,
                    root=root,
                    matcher=PathPatternMatcher(()),
                    collect_users=False,
                    user_max_depth=0,
                    large_file_limit=0,
                    record_max_depth=None,
                )
                with (
                    patch("qdu.scanner._MAX_OPEN_LISTINGS", 4),
                    patch("qdu.scanner.os.scandir", side_effect=CountedListing),
                ):
                    summary = scanner.scan("deep")
            finally:
                connection.close()

        self.assertTrue(summary.complete)
        self.assertEqual(summary.totals.file_count, 2)
        self.assertEqual(summary.totals.directory_count, depth + 1)
        self.assertLessEqual(open_listings[1], 5)
        self.assertEqual(open_listings[0], 0)
