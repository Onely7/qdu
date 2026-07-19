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


class SnapshotCommandTest(QduIntegrationTestBase):
    def test_snapshot_show_list_json(self) -> None:
        self.write_file("a/large.bin", 8192)
        self.write_file("b/small.bin", 1024)
        status, _, _ = self.snapshot("--file-top", "10")
        self.assertEqual(status, 0)
        status, output, _ = self.run_qdu("show", "--format", "json", "-L", "2")
        self.assertEqual(status, 0)
        payload = json.loads(output)
        self.assertEqual(payload["scope"], ".")
        self.assertEqual(payload["entries"][0]["path"], "a")
        status, output, _ = self.run_qdu("list", "--format", "json")
        self.assertEqual(status, 0)
        self.assertEqual(len(json.loads(output)["snapshots"]), 1)

    def test_scope_share_uses_under_directory(self) -> None:
        self.write_file("scope/a.bin", 4096)
        self.write_file("scope/child/b.bin", 4096)
        self.write_file("outside/c.bin", 65536)
        self.snapshot()
        status, output, _ = self.run_qdu(
            "show", "--under", "scope", "-L", "1", "--format", "json"
        )
        self.assertEqual(status, 0)
        payload = json.loads(output)
        child = next(item for item in payload["entries"] if item["path"] == "child")
        self.assertGreater(child["share_percent"], 0)
        self.assertLessEqual(child["share_percent"], 100)

    def test_apparent_metric_is_consistent(self) -> None:
        self.write_file("a/data.bin", 12345)
        self.snapshot()
        status, output, _ = self.run_qdu(
            "show", "--metric", "apparent_bytes", "--format", "json"
        )
        self.assertEqual(status, 0)
        payload = json.loads(output)
        self.assertEqual(payload["metric"], "apparent_bytes")
        self.assertEqual(
            payload["entries"][0]["metric_value"],
            payload["entries"][0]["apparent_bytes"],
        )

    def test_diff_orders_growth_shrink_absolute_current(self) -> None:
        self.write_file("grow/file", 4096)
        self.write_file("shrink/file", 16384)
        self.snapshot()
        self.write_file("grow/file", 32768)
        self.write_file("shrink/file", 1)
        self.snapshot()
        for order in ("growth", "shrink", "absolute", "current"):
            status, output, _ = self.run_qdu(
                "diff", "--order", order, "--format", "json"
            )
            self.assertEqual(status, 0)
            self.assertEqual(json.loads(output)["order"], order)
        status, output, _ = self.run_qdu("diff", "--growth-only", "--format", "json")
        self.assertTrue(
            all(item["change_bytes"] > 0 for item in json.loads(output)["entries"])
        )
        status, output, _ = self.run_qdu("diff", "--shrink-only", "--format", "json")
        self.assertTrue(
            all(item["change_bytes"] < 0 for item in json.loads(output)["entries"])
        )

    def test_hard_links_are_deduplicated(self) -> None:
        original = self.write_file("a/original", 4096)
        os.link(original, self.root / "a/link")
        self.snapshot("--with-users")
        record = self.index().snapshots[-1]
        paths = self.profile_paths()
        database = paths.snapshots / record.filename
        with sqlite3.connect(database) as connection:
            root_row = connection.execute(
                "SELECT file_count FROM directories WHERE path='.'"
            ).fetchone()
        self.assertEqual(root_row[0], 1)

    def test_exclusion_contract_is_shared(self) -> None:
        self.write_file("keep/data", 1024)
        self.write_file("nested/cache/data", 16384)
        self.snapshot("--exclude", "cache", "--with-users")
        status, output, _ = self.run_qdu("show", "-L", "5", "--format", "json")
        paths = [item["path"] for item in json.loads(output)["entries"]]
        self.assertFalse(any("cache" in path for path in paths))
        status, output, _ = self.run_qdu("users", "--format", "json")
        payload = json.loads(output)
        self.assertFalse(
            any(
                "cache" in directory["path"]
                for user in payload["users"]
                for directory in user["directories"]
            )
        )

    def test_tabs_newlines_and_unicode_are_stored_safely(self) -> None:
        self.write_file("日本語/line\nbreak/tab\tfile", 1024)
        self.snapshot()
        status, output, _ = self.run_qdu("show", "-L", "5", "--format", "json")
        self.assertEqual(status, 0)
        paths = [item["path"] for item in json.loads(output)["entries"]]
        self.assertTrue(any("\n" in path or "\t" in path for path in paths))
        status, output, _ = self.run_qdu("show", "-L", "5", "--style", "plain")
        self.assertNotIn("line\nbreak", output)
        self.assertIn("line\\nbreak", output)

    def test_incomplete_snapshot_does_not_replace_latest_complete(self) -> None:
        self.write_file("ok/file", 1024)
        self.snapshot()
        first = self.index().latest_complete
        denied = self.root / "denied"
        denied.mkdir()
        real_scandir = os.scandir

        def fake_scandir(path):
            if Path(path) == denied:
                raise PermissionError("simulated permission denial")
            return real_scandir(path)

        with patch("qdu.scanner.os.scandir", side_effect=fake_scandir):
            status, _, _ = self.snapshot()
        self.assertEqual(status, 3)
        index = self.index()
        self.assertEqual(index.latest_complete, first)
        self.assertNotEqual(index.latest_any, first)
        status, output, _ = self.run_qdu("show", "--format", "json")
        self.assertEqual(json.loads(output)["snapshot"]["snapshot_id"], first)

    def test_profile_root_is_not_silently_changed(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        other = self.base / "other"
        other.mkdir()
        status, _, error = self.run_qdu("snapshot", "--path", str(other), "--quiet")
        self.assertEqual(status, 2)
        self.assertIn("already bound", error)

    def test_pruning_keeps_requested_count(self) -> None:
        self.write_file("data", 1024)
        for size in (1024, 2048, 4096):
            self.write_file("data", size)
            self.snapshot("--keep-snapshots", "2")
        self.assertEqual(len(self.index().snapshots), 2)

    def test_fatal_snapshot_error_cleans_temporary_database(self) -> None:
        missing = self.base / "missing"
        status, _, _ = self.run_qdu("snapshot", "--path", str(missing), "--quiet")
        self.assertEqual(status, 2)
        paths = self.profile_paths()
        if paths.temporary.exists():
            self.assertEqual(list(paths.temporary.iterdir()), [])

    def test_archived_snapshot_verifies_and_participates_in_diff(self) -> None:
        self.write_file("data/file", 1024)
        self.snapshot()
        self.write_file("data/file", 16384)
        self.snapshot()
        self.run_qdu("compact", "--older-than", "0s", "--keep-latest", "1")
        status, _, _ = self.run_qdu("verify", "--all")
        self.assertEqual(status, 0)
        status, output, _ = self.run_qdu("diff", "--format", "json")
        self.assertEqual(status, 0)
        self.assertGreater(json.loads(output)["entries"][0]["change_bytes"], 0)

    def test_quiet_incomplete_snapshot_still_reports_errors_and_errors_command(
        self,
    ) -> None:
        self.write_file("ok/file", 1024)
        denied = self.root / "denied"
        denied.mkdir()
        real_scandir = os.scandir

        def fake_scandir(path):
            if Path(path) == denied:
                raise PermissionError("simulated permission denial")
            return real_scandir(path)

        with patch("qdu.scanner.os.scandir", side_effect=fake_scandir):
            status, output, error = self.snapshot()
        self.assertEqual(status, 3)
        self.assertEqual(output, "")
        self.assertIn("could not be read", error)
        status, output, _ = self.run_qdu("errors", "--format", "json")
        self.assertEqual(status, 0)
        payload = json.loads(output)
        self.assertEqual(payload["error_count"], 1)
        self.assertEqual(payload["errors"][0]["path"], "denied")
