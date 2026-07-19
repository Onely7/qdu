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

from tests.support import QduIntegrationTestBase


class OperationsCommandTest(QduIntegrationTestBase):
    def test_cross_filesystems_snapshot_option_is_recorded(self) -> None:
        self.write_file("data/file", 1024)
        status, _, _ = self.snapshot("--cross-filesystems")
        self.assertEqual(status, 0)
        record = self.index().snapshots[-1]
        database = self.profile_paths().snapshots / record.filename
        with sqlite3.connect(database) as connection:
            metadata = snapshot_metadata(connection)
        self.assertEqual(metadata["cross_filesystems"], "true")
        status, output, _ = self.run_qdu("show", "--format", "json")
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output)["filesystem"]["mode"], "cross-filesystems")

    def test_profile_can_persist_cross_filesystems(self) -> None:
        status, _, _ = self.run_qdu(
            "profile",
            "add",
            "shared",
            "--path",
            str(self.root),
            "--cross-filesystems",
        )
        self.assertEqual(status, 0)
        status, output, _ = self.run_qdu(
            "profile", "show", "shared", "--format", "json"
        )
        self.assertEqual(status, 0)
        self.assertTrue(json.loads(output)["cross_filesystems"])
        status, _, _ = self.run_qdu("snapshot", "--profile", "shared", "--quiet")
        self.assertEqual(status, 0)
        with patch.dict(os.environ, self.environment, clear=False):
            index = IndexRepository(ProfilePaths.for_profile("shared")).load()
            record = index.snapshots[-1]
            database = ProfilePaths.for_profile("shared").snapshots / record.filename
        with sqlite3.connect(database) as connection:
            metadata = snapshot_metadata(connection)
        self.assertEqual(metadata["cross_filesystems"], "true")

        status, _, _ = self.run_qdu(
            "snapshot", "--profile", "shared", "--one-file-system", "--quiet"
        )
        self.assertEqual(status, 0)
        with patch.dict(os.environ, self.environment, clear=False):
            index = IndexRepository(ProfilePaths.for_profile("shared")).load()
            record = index.snapshots[-1]
            database = ProfilePaths.for_profile("shared").snapshots / record.filename
        with sqlite3.connect(database) as connection:
            metadata = snapshot_metadata(connection)
        self.assertEqual(metadata["cross_filesystems"], "false")

    def test_profile_and_config_commands(self) -> None:
        status, output, _ = self.run_qdu("config", "init")
        self.assertEqual(status, 0)
        status, _, _ = self.run_qdu(
            "profile", "add", "data", "--path", str(self.root), "--exclude", ".git"
        )
        self.assertEqual(status, 0)
        status, output, _ = self.run_qdu("profile", "show", "data", "--format", "json")
        payload = json.loads(output)
        self.assertEqual(payload["name"], "data")
        self.assertIn(".git", payload["exclude"])
        status, output, _ = self.run_qdu("profile", "list", "--style", "plain")
        self.assertIn("data", output)

    def test_verify_detects_corruption(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        status, _, _ = self.run_qdu("verify", "--all")
        self.assertEqual(status, 0)
        record = self.index().snapshots[-1]
        path = self.profile_paths().snapshots / record.filename
        with path.open("ab") as handle:
            handle.write(b"corruption")
        status, _, error = self.run_qdu("verify", "--all")
        self.assertEqual(status, 6)
        self.assertIn("failed verification", error.lower())

    def test_repair_rebuilds_missing_index(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        paths = self.profile_paths()
        paths.index.unlink()
        status, _, _ = self.run_qdu("repair")
        self.assertEqual(status, 0)
        self.assertEqual(len(self.index().snapshots), 1)

    def test_compact_archives_and_archived_snapshot_remains_readable(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        self.write_file("data", 8192)
        self.snapshot()
        status, _, _ = self.run_qdu(
            "compact", "--older-than", "0s", "--keep-latest", "1"
        )
        self.assertEqual(status, 0)
        index = self.index()
        self.assertTrue(index.snapshots[0].archived)
        status, output, _ = self.run_qdu(
            "show", "--snapshot", "previous", "--format", "json"
        )
        self.assertEqual(status, 0)
        self.assertEqual(
            json.loads(output)["snapshot"]["snapshot_id"],
            index.snapshots[0].snapshot_id,
        )

    def test_check_returns_alert_exit_code(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        status, output, _ = self.run_qdu(
            "check", "--disk-usage-over", "0", "--format", "json"
        )
        self.assertEqual(status, 10)
        self.assertFalse(json.loads(output)["passed"])

    def test_lock_is_process_safe(self) -> None:
        self.write_file("data", 1024)
        with patch.dict(os.environ, self.environment, clear=False):
            paths = ProfilePaths.for_profile("default")
            with ProfileLock(paths.lock, "test holder"):
                status, _, error = self.snapshot()
        self.assertEqual(status, 5)
        self.assertIn("active", error)

    def test_growth_check_and_exclusion_difference_warning(self) -> None:
        self.write_file("data/file", 1024)
        self.snapshot("--exclude", ".git")
        self.write_file("data/file", 32768)
        self.snapshot("--exclude", "node_modules")
        status, output, error = self.run_qdu("diff", "--format", "json")
        self.assertEqual(status, 0)
        self.assertIn("exclusion patterns differ", error)
        status, output, _ = self.run_qdu(
            "check", "--path", "data", "--growth-over", "1B", "--format", "json"
        )
        self.assertEqual(status, 10)
        self.assertFalse(json.loads(output)["passed"])

    def test_unlock_removes_stale_metadata(self) -> None:
        paths = self.profile_paths()
        paths.ensure()
        paths.lock.write_text('{"pid": 999999, "host": "stale"}\n', encoding="utf-8")
        status, output, _ = self.run_qdu("unlock")
        self.assertEqual(status, 0)
        self.assertFalse(paths.lock.exists())
        self.assertIn("removed", output.lower())

    def test_repair_dry_run_preserves_index(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        paths = self.profile_paths()
        before = paths.index.read_bytes()
        status, output, _ = self.run_qdu("repair", "--dry-run")
        self.assertEqual(status, 0)
        self.assertEqual(paths.index.read_bytes(), before)
        self.assertIn("dry-run", output)

    def test_malformed_configuration_is_a_usage_error(self) -> None:
        config_path = self.config / "qdu" / "config.ini"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(
            "[defaults]\nkeep_snapshots = not-a-number\n", encoding="utf-8"
        )
        status, _, error = self.run_qdu("doctor")
        self.assertEqual(status, 2)
        self.assertIn("invalid configuration", error)
