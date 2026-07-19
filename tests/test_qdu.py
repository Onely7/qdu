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


class QduIntegrationTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.base = Path(self.temporary.name)
        self.home = self.base / "home"
        self.state = self.base / "state"
        self.config = self.base / "config"
        self.root = self.base / "root"
        for path in (self.home, self.state, self.config, self.root):
            path.mkdir(parents=True)
        self.environment = {
            "HOME": str(self.home),
            "XDG_STATE_HOME": str(self.state),
            "XDG_CONFIG_HOME": str(self.config),
            "QDU_PROFILE": "default",
        }

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def run_qdu(self, *arguments: str) -> tuple[int, str, str]:
        stdout = io.StringIO()
        stderr = io.StringIO()
        with patch.dict(os.environ, self.environment, clear=False):
            with redirect_stdout(stdout), redirect_stderr(stderr):
                status = main(list(arguments))
        return status, stdout.getvalue(), stderr.getvalue()

    def write_file(self, relative: str, size: int, byte: bytes = b"x") -> Path:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(byte * size)
        return path

    def snapshot(self, *extra: str) -> tuple[int, str, str]:
        return self.run_qdu("snapshot", "--path", str(self.root), "--quiet", *extra)

    def index(self):
        with patch.dict(os.environ, self.environment, clear=False):
            return IndexRepository(ProfilePaths.for_profile("default")).load()

    def profile_paths(self):
        with patch.dict(os.environ, self.environment, clear=False):
            return ProfilePaths.for_profile("default")

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

    def test_capacity_limit_is_reported_and_warns_when_exceeded(self) -> None:
        self.write_file("data.bin", 8192)
        self.snapshot()
        status, output, error = self.run_qdu(
            "show", "--capacity-limit", "1B", "--format", "json"
        )
        self.assertEqual(status, 0)
        payload = json.loads(output)
        self.assertTrue(payload["capacity"]["exceeded"])
        self.assertGreater(payload["capacity"]["usage_percent"], 100)
        self.assertIn("capacity limit exceeded", error)
        status, output, _ = self.run_qdu(
            "show", "--capacity-limit", "1B", "--format", "tsv"
        )
        self.assertEqual(status, 0)
        self.assertIn("capacity_limit_bytes", output.splitlines()[0])

    def test_profile_capacity_user_requires_user_collection(self) -> None:
        status, _, error = self.run_qdu(
            "profile",
            "add",
            "invalid-user-limit",
            "--path",
            str(self.root),
            "--capacity-limit",
            "1GiB",
            "--capacity-user",
            str(os.getuid()),
        )
        self.assertEqual(status, 2)
        self.assertIn("collect_users", error)

    def test_profile_capacity_limit_is_persistent_and_checkable(self) -> None:
        self.write_file("data.bin", 8192)
        status, _, error = self.run_qdu(
            "profile",
            "add",
            "limited",
            "--path",
            str(self.root),
            "--capacity-limit",
            "1B",
        )
        self.assertEqual(status, 0, error)
        status, _, error = self.run_qdu("snapshot", "--profile", "limited", "--quiet")
        self.assertEqual(status, 0, error)
        status, output, error = self.run_qdu(
            "show", "--profile", "limited", "--format", "json"
        )
        self.assertEqual(status, 0)
        self.assertTrue(json.loads(output)["capacity"]["exceeded"])
        status, output, error = self.run_qdu(
            "check", "--profile", "limited", "--format", "json"
        )
        self.assertEqual(status, 10, error)
        payload = json.loads(output)
        self.assertFalse(payload["passed"])
        self.assertEqual(payload["checks"][0]["name"], "operational_capacity")

    def test_capacity_user_uses_owner_statistics(self) -> None:
        self.write_file("owned/data.bin", 8192)
        self.snapshot("--with-users")
        status, output, error = self.run_qdu(
            "show",
            "--capacity-limit",
            "1B",
            "--capacity-user",
            str(os.getuid()),
            "--format",
            "json",
        )
        self.assertEqual(status, 0, error)
        payload = json.loads(output)
        self.assertIn(f"uid {os.getuid()}", payload["capacity"]["target"])
        self.assertTrue(payload["capacity"]["exceeded"])

    def test_no_capacity_limit_suppresses_saved_user_limit(self) -> None:
        self.write_file("data.bin", 8192)
        status, _, error = self.run_qdu(
            "profile",
            "add",
            "limited-user",
            "--path",
            str(self.root),
            "--with-users",
            "--capacity-limit",
            "1B",
            "--capacity-user",
            str(os.getuid()),
        )
        self.assertEqual(status, 0, error)
        status, _, error = self.run_qdu(
            "snapshot", "--profile", "limited-user", "--quiet"
        )
        self.assertEqual(status, 0, error)
        status, output, error = self.run_qdu(
            "show",
            "--profile",
            "limited-user",
            "--no-capacity-limit",
            "--format",
            "json",
        )
        self.assertEqual(status, 0, error)
        self.assertIsNone(json.loads(output)["capacity"])

    def test_capacity_user_requires_owner_statistics(self) -> None:
        self.write_file("data.bin", 1024)
        self.snapshot()
        status, _, error = self.run_qdu(
            "show",
            "--capacity-limit",
            "1GiB",
            "--capacity-user",
            str(os.getuid()),
        )
        self.assertEqual(status, 2)
        self.assertIn("--with-users", error)

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

    def test_users_inodes_and_owner_directories(self) -> None:
        self.write_file("alice/data.bin", 8192)
        self.snapshot("--with-users")
        status, output, _ = self.run_qdu("users", "--format", "json")
        self.assertEqual(status, 0)
        users = json.loads(output)["users"]
        self.assertGreaterEqual(len(users), 1)
        self.assertIn("directories", users[0])
        status, output, _ = self.run_qdu("inodes", "--users", "--format", "json")
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output)["metric"], "inode_count")

    def test_large_files_snapshot_and_live(self) -> None:
        self.write_file("one.bin", 4096)
        self.write_file("two.bin", 8192)
        self.snapshot("--file-top", "10")
        status, output, _ = self.run_qdu("files", "--format", "json")
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output)["files"][0]["path"], "two.bin")
        status, output, _ = self.run_qdu("files", "--live", "--format", "json")
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output)["files"][0]["path"], "two.bin")

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

    def test_doctor_explain_and_inodes_json(self) -> None:
        self.write_file("a/b/data", 1024)
        self.snapshot()
        status, output, _ = self.run_qdu("doctor", "--format", "json")
        self.assertIn(status, (0, 1))
        self.assertIn("items", json.loads(output))
        status, output, _ = self.run_qdu("explain", "a", "--format", "json")
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output)["scope"], "a")
        status, output, _ = self.run_qdu("inodes", "--format", "json")
        self.assertEqual(status, 0)
        self.assertIn("entries", json.loads(output))

    def test_pruning_keeps_requested_count(self) -> None:
        self.write_file("data", 1024)
        for size in (1024, 2048, 4096):
            self.write_file("data", size)
            self.snapshot("--keep-snapshots", "2")
        self.assertEqual(len(self.index().snapshots), 2)

    def test_stale_directory_annotations_colors_and_filtering(self) -> None:
        now = time.time()

        def create_aged_directory(name: str, days: int) -> None:
            path = self.write_file(f"{name}/data.bin", 1024)
            timestamp = now - (days * 24 * 60 * 60)
            os.utime(path, (timestamp, timestamp))
            os.utime(path.parent, (timestamp, timestamp))

        create_aged_directory("fresh", 10)
        create_aged_directory("old30", 31)
        create_aged_directory("old90", 91)
        create_aged_directory("old180", 181)
        create_aged_directory("old365", 366)

        old_parent = self.root / "parent-with-recent-child"
        recent_file = self.write_file(
            "parent-with-recent-child/nested/recent.bin", 1024
        )
        old_timestamp = now - (400 * 24 * 60 * 60)
        recent_timestamp = now - (5 * 24 * 60 * 60)
        os.utime(old_parent, (old_timestamp, old_timestamp))
        os.utime(old_parent / "nested", (old_timestamp, old_timestamp))
        os.utime(recent_file, (recent_timestamp, recent_timestamp))

        self.snapshot("--file-top", "20")
        status, output, _ = self.run_qdu(
            "show", "--stale", "--format", "json", "-L", "2", "-n", "100"
        )
        self.assertEqual(status, 0)
        entries = {item["path"]: item for item in json.loads(output)["entries"]}
        self.assertEqual(entries["fresh"]["stale_level"], "recent")
        self.assertEqual(entries["old30"]["stale_level"], "notice")
        self.assertEqual(entries["old90"]["stale_level"], "warning")
        self.assertEqual(entries["old180"]["stale_level"], "elevated")
        self.assertEqual(entries["old365"]["stale_level"], "critical")
        self.assertEqual(entries["parent-with-recent-child"]["stale_level"], "recent")

        status, output, _ = self.run_qdu(
            "show", "--stale-only", "180", "--format", "json", "-L", "2", "-n", "100"
        )
        self.assertEqual(status, 0)
        filtered = {item["path"] for item in json.loads(output)["entries"]}
        self.assertIn("old180", filtered)
        self.assertIn("old365", filtered)
        self.assertNotIn("old90", filtered)
        self.assertNotIn("parent-with-recent-child", filtered)

        status, output, _ = self.run_qdu(
            "show",
            "--stale",
            "--color",
            "always",
            "--style",
            "plain",
            "-L",
            "1",
            "-n",
            "100",
        )
        self.assertEqual(status, 0)
        self.assertIn("\033[34m", output)
        self.assertIn("\033[33m", output)
        self.assertIn("\033[38;5;208m", output)
        self.assertIn("\033[31m", output)

    def test_stale_file_annotations_and_filtering(self) -> None:
        now = time.time()
        recent = self.write_file("recent.bin", 2048)
        old = self.write_file("old.bin", 4096)
        os.utime(recent, (now - 10 * 86400, now - 10 * 86400))
        os.utime(old, (now - 120 * 86400, now - 120 * 86400))
        self.snapshot("--file-top", "10")

        status, output, _ = self.run_qdu("files", "--stale", "--format", "json")
        self.assertEqual(status, 0)
        files = {item["path"]: item for item in json.loads(output)["files"]}
        self.assertEqual(files["recent.bin"]["stale_level"], "recent")
        self.assertEqual(files["old.bin"]["stale_level"], "warning")

        status, output, _ = self.run_qdu(
            "files", "--stale-only", "90", "--format", "json"
        )
        self.assertEqual(status, 0)
        self.assertEqual(
            [item["path"] for item in json.loads(output)["files"]], ["old.bin"]
        )

    def test_zero_result_is_explicit_and_json_is_empty(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        status, output, _ = self.run_qdu(
            "show", "--min-size", "1TiB", "--style", "plain"
        )
        self.assertEqual(status, 0)
        self.assertIn("該当する項目はありません", output)
        status, output, _ = self.run_qdu(
            "show", "--min-size", "1TiB", "--format", "json"
        )
        self.assertEqual(json.loads(output)["entries"], [])

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

    def test_repair_dry_run_preserves_index(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        paths = self.profile_paths()
        before = paths.index.read_bytes()
        status, output, _ = self.run_qdu("repair", "--dry-run")
        self.assertEqual(status, 0)
        self.assertEqual(paths.index.read_bytes(), before)
        self.assertIn("dry-run", output)

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

    def test_malformed_configuration_is_a_usage_error(self) -> None:
        config_path = self.config / "qdu" / "config.ini"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(
            "[defaults]\nkeep_snapshots = not-a-number\n", encoding="utf-8"
        )
        status, _, error = self.run_qdu("doctor")
        self.assertEqual(status, 2)
        self.assertIn("invalid configuration", error)

    def test_browse_streams_paths_to_optional_fzf(self) -> None:
        self.write_file("a/child/file", 1024)
        self.snapshot()
        bin_dir = self.base / "bin"
        bin_dir.mkdir()
        fzf = bin_dir / "fzf"
        fzf.write_text(
            "#!/usr/bin/env sh\nprintf 'a\n'\ncat >/dev/null\n", encoding="utf-8"
        )
        fzf.chmod(0o755)
        self.environment["PATH"] = f"{bin_dir}:{os.environ.get('PATH', '')}"
        status, output, _ = self.run_qdu("browse", "--format", "json")
        self.assertEqual(status, 0)
        self.assertEqual(json.loads(output)["scope"], "a")


class UtilityTest(unittest.TestCase):
    def test_units(self) -> None:
        self.assertEqual(parse_size("1KiB"), 1024)
        self.assertEqual(parse_size("1GB"), 1000**3)
        self.assertEqual(parse_duration("2h"), 7200)
        self.assertEqual(format_bytes(1536), "1.5KiB")

    def test_stale_thresholds(self) -> None:
        thresholds = StaleThresholds.parse("30,90,180,365")
        self.assertEqual(
            assess_staleness(
                0, reference_epoch=30 * 86400, thresholds=thresholds
            ).level,
            StaleLevel.NOTICE,
        )
        with self.assertRaises(ValueError):
            StaleThresholds.parse("30,30,180,365")

    def test_capacity_assessment(self) -> None:
        within = assess_capacity(target="test", limit_bytes=100, used_bytes=25)
        self.assertFalse(within.exceeded)
        self.assertEqual(within.remaining_bytes, 75)
        self.assertEqual(within.usage_percent, 25.0)
        exceeded = assess_capacity(target="test", limit_bytes=100, used_bytes=125)
        self.assertTrue(exceeded.exceeded)
        self.assertEqual(exceeded.excess_bytes, 25)

    def test_pattern_contract(self) -> None:
        matcher = PathPatternMatcher(("cache", "models/*.bin"))
        self.assertTrue(matcher.matches("a/cache/file"))
        self.assertTrue(matcher.matches("models/x.bin"))
        self.assertFalse(matcher.matches("models/sub/x.bin"))
        self.assertFalse(matcher.matches("cached/file"))

    def test_unicode_width_and_control_escaping(self) -> None:
        self.assertEqual(display_width("abc"), 3)
        self.assertEqual(display_width("日本"), 4)
        self.assertLessEqual(display_width(truncate("日本語ファイル", 6)), 6)
        self.assertEqual(display_safe("a\tb\nc"), "a\\tb\\nc")


class FilesystemBoundaryTest(unittest.TestCase):
    def test_cross_filesystems_controls_mount_descent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
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


class DistributionArtifactTest(unittest.TestCase):
    def test_zipapp_is_the_tested_release_artifact(self) -> None:
        project = Path(__file__).resolve().parents[1]
        subprocess.run(
            [sys.executable, str(project / "tools" / "build_zipapp.py")], check=True
        )
        artifact = project / "dist" / "qdu"
        result = subprocess.run(
            [str(artifact), "--version"], text=True, capture_output=True, check=True
        )
        self.assertIn("qdu 0.0.1", result.stdout)
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root = base / "root"
            root.mkdir()
            (root / "file").write_text("data", encoding="utf-8")
            environment = os.environ.copy()
            environment.update(
                {
                    "HOME": str(base / "home"),
                    "XDG_STATE_HOME": str(base / "state"),
                    "XDG_CONFIG_HOME": str(base / "config"),
                }
            )
            (base / "home").mkdir()
            subprocess.run(
                [str(artifact), "snapshot", "--path", str(root), "--quiet"],
                env=environment,
                check=True,
            )
            output = subprocess.run(
                [str(artifact), "show", "--format", "json"],
                env=environment,
                text=True,
                capture_output=True,
                check=True,
            ).stdout
            self.assertEqual(json.loads(output)["scope"], ".")


if __name__ == "__main__":
    unittest.main()
