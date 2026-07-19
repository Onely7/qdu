from __future__ import annotations

import io
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

from qdu.capacity import assess_capacity
from qdu.cli import main
from qdu.locking import ProfileLock
from qdu.patterns import PathPatternMatcher
from qdu.render import display_safe, display_width, truncate
from qdu.staleness import StaleLevel, StaleThresholds, assess_staleness
from qdu.scanner import FilesystemScanner
from qdu.storage import (
    IndexRepository,
    ProfilePaths,
    create_snapshot_database,
    snapshot_metadata,
)
from qdu.units import format_bytes, parse_duration, parse_size


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
                    fields = list(value)
                    fields[2] = fake_device
                    return os.stat_result(fields)

            class ScandirWrapper:
                def __init__(self, entries):
                    self._iterator = iter(entries)

                def __iter__(self):
                    return self

                def __next__(self):
                    return next(self._iterator)

                def close(self):
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

