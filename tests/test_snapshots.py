from __future__ import annotations

import errno
import json
import os
import sqlite3
from pathlib import Path
from unittest.mock import patch

from tests.support import QduIntegrationTestBase


class SnapshotCommandTest(QduIntegrationTestBase):
    def test_rejects_snapshot_filenames_outside_the_snapshot_directory(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        paths = self.profile_paths()
        payload = json.loads(paths.index.read_text(encoding="utf-8"))
        payload["snapshots"][0]["filename"] = "../outside.sqlite3"
        paths.index.write_text(json.dumps(payload), encoding="utf-8")

        status, _, error = self.run_qdu("list")

        self.assertEqual(status, 4)
        self.assertIn("invalid profile index", error)

    def test_snapshot_shows_no_progress_without_a_terminal(self) -> None:
        self.write_file("data", 1024)
        status, output, error = self.run_qdu("snapshot", "--path", str(self.root))
        self.assertEqual(status, 0, error)
        self.assertIn("Snapshot saved", output)
        self.assertNotIn("\r", error)
        self.assertNotIn("Scanning", error)

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

    def test_list_reports_the_disk_space_of_each_snapshot(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        self.snapshot()
        paths = self.profile_paths()
        expected = {
            item.snapshot_id: (paths.snapshots / item.filename).stat().st_size
            for item in self.index().snapshots
        }
        status, output, error = self.run_qdu("list", "--format", "json")
        self.assertEqual(status, 0, error)
        payload = json.loads(output)
        self.assertEqual(
            {
                item["snapshot_id"]: item["stored_bytes"]
                for item in payload["snapshots"]
            },
            expected,
        )
        self.assertEqual(payload["stored_bytes_total"], sum(expected.values()))
        status, output, _ = self.run_qdu("list", "--format", "tsv")
        self.assertEqual(output.splitlines()[0].split("\t")[-1], "stored_bytes")
        status, output, _ = self.run_qdu("list", "--style", "plain")
        self.assertIn("Stored", output)
        self.assertIn("2 snapshot(s) use", output)

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

    def test_under_scope_is_case_sensitive_and_literal(self) -> None:
        self.write_file("data/lower/file", 8192)
        self.write_file("data_x/under/file", 16384)
        # macOS volumes are usually case-insensitive; there "Data" is "data".
        case_sensitive = not (self.root / "DATA").exists()
        if case_sensitive:
            self.write_file("Data/upper/file", 4096)
        self.snapshot("--file-top", "10")
        self.write_file("data/lower/file", 32768)
        if case_sensitive:
            self.write_file("Data/upper/file", 16384)
        self.snapshot("--file-top", "10")

        def paths(*arguments: str, key: str = "entries") -> list[str]:
            status, output, error = self.run_qdu(*arguments, "--format", "json")
            self.assertEqual(status, 0, error)
            return [item["path"] for item in json.loads(output)[key]]

        self.assertEqual(paths("show", "--under", "data", "-L", "2"), ["lower"])
        self.assertEqual(paths("explain", "data"), ["lower"])
        self.assertEqual(paths("diff", "--under", "data", "-L", "2"), ["lower"])
        self.assertEqual(
            paths("files", "--under", "data", key="files"), ["data/lower/file"]
        )
        if case_sensitive:
            self.assertEqual(paths("show", "--under", "Data", "-L", "2"), ["upper"])
            self.assertEqual(paths("diff", "--under", "Data", "-L", "2"), ["upper"])

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

    def test_diff_explains_that_it_needs_two_snapshots(self) -> None:
        self.write_file("file", 1024)
        self.snapshot()
        status, _, error = self.run_qdu("diff")
        self.assertEqual(status, 2)
        self.assertIn("needs two complete snapshots", error)
        self.snapshot()
        first = self.index().snapshots[0].snapshot_id
        status, _, error = self.run_qdu("diff", "--to", first)
        self.assertEqual(status, 2)
        self.assertIn(f"no complete snapshot exists before {first}", error)

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
            all(item["change"] > 0 for item in json.loads(output)["entries"])
        )
        status, output, _ = self.run_qdu("diff", "--shrink-only", "--format", "json")
        self.assertTrue(
            all(item["change"] < 0 for item in json.loads(output)["entries"])
        )

    def test_diff_compares_the_selected_metric(self) -> None:
        self.write_file("many/seed", 1024)
        self.write_file("big/data", 4096)
        self.snapshot()
        for index in range(5):
            self.write_file(f"many/new-{index}", 0)
        self.write_file("big/data", 1 << 20)
        self.snapshot()

        def entries(*options: str) -> dict[str, dict]:
            status, output, error = self.run_qdu("diff", *options, "--format", "json")
            self.assertEqual(status, 0, error)
            payload = json.loads(output)
            return payload["metric"], {
                item["path"]: item for item in payload["entries"]
            }

        metric, inodes = entries("--metric", "inodes", "--order", "growth")
        self.assertEqual(metric, "inode_count")
        self.assertEqual(next(iter(inodes)), "many")
        self.assertEqual(inodes["many"]["change"], 5)
        self.assertEqual(inodes["many"]["change_percent"], 5 * 100 / 2)
        metric, apparent = entries("--metric", "apparent_bytes")
        self.assertEqual(metric, "apparent_bytes")
        self.assertEqual(apparent["big"]["change"], (1 << 20) - 4096)
        metric, allocated = entries()
        self.assertEqual(metric, "allocated_bytes")
        self.assertEqual(next(iter(allocated)), "big")
        status, output, error = self.run_qdu(
            "diff", "--metric", "inodes", "--style", "plain"
        )
        self.assertEqual(status, 0, error)
        self.assertIn("+5", output)
        self.assertIn("+250.0%", output)
        with self.assertRaises(SystemExit):
            self.run_qdu("diff", "--metric", "files")

    def test_snapshot_summary_counts_one_file_in_the_singular(self) -> None:
        self.write_file("only", 1024)
        status, output, error = self.run_qdu(
            "snapshot", "--path", str(self.root), "--style", "plain"
        )
        self.assertEqual(status, 0, error)
        self.assertIn(" in 1 file and 1 directory\n", output)

    def test_reports_are_compact_and_diffs_list_changes_only(self) -> None:
        self.write_file("grow/file", 1024)
        self.write_file("same/file", 1024)
        status, output, error = self.run_qdu(
            "snapshot", "--path", str(self.root), "--style", "plain"
        )
        self.assertEqual(status, 0, error)
        self.assertIn("2 files and 3 directories", output)
        self.assertNotIn("Skipped mounts", output)
        self.write_file("grow/file", 65536)
        self.snapshot()

        status, output, _ = self.run_qdu("show", "--style", "plain")
        header = output.split("\n\n", 1)[0].splitlines()
        self.assertEqual(
            [line.split()[0] for line in header[1:]],
            ["Root", "Snapshot", "Scope", "Disk"],
        )
        status, output, _ = self.run_qdu("diff", "--format", "json")
        self.assertEqual(
            [item["path"] for item in json.loads(output)["entries"]], ["grow"]
        )
        status, output, _ = self.run_qdu("diff", "--under", "same", "--style", "plain")
        self.assertIn("No changes in this scope.", output)

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
        _, output, _ = self.run_qdu("show", "-L", "5", "--format", "json")
        paths = [item["path"] for item in json.loads(output)["entries"]]
        self.assertFalse(any("cache" in path for path in paths))
        _, output, _ = self.run_qdu("users", "--format", "json")
        payload = json.loads(output)
        self.assertFalse(
            any(
                "cache" in directory["path"]
                for user in payload["users"]
                for directory in user["directories"]
            )
        )

    def test_exclusion_patterns_anchor_and_select_directories(self) -> None:
        self.write_file("build/out.o", 4096)
        self.write_file("src/build/keep.o", 4096)
        self.write_file("logs/app.log", 4096)
        self.write_file("other/logs", 4096)
        self.write_file("run/models/weights.bin", 4096)
        self.write_file("run/models/notes.txt", 4096)
        self.write_file("cache/blob", 4096)
        status, _, error = self.snapshot(
            "--file-top",
            "20",
            "--exclude",
            "/build",
            "--exclude",
            "logs/",
            "--exclude",
            "**/models/*.bin",
            "--exclude",
            "cache/**",
        )
        self.assertEqual(status, 0, error)
        _, output, _ = self.run_qdu("show", "-L", "5", "--format", "json")
        directories = {item["path"] for item in json.loads(output)["entries"]}
        self.assertEqual(
            directories, {"src", "src/build", "other", "run", "run/models", "cache"}
        )
        _, output, _ = self.run_qdu("files", "--format", "json")
        files = {item["path"] for item in json.loads(output)["files"]}
        self.assertEqual(
            files, {"src/build/keep.o", "other/logs", "run/models/notes.txt"}
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

    def test_undecodable_file_names_are_stored_with_escapes(self) -> None:
        directory = os.fsencode(self.root) + b"/latin1-\xe9t\xe9"
        try:
            os.mkdir(directory)
        except OSError as exc:  # APFS, for example, rejects non-UTF-8 names.
            self.skipTest(f"filesystem rejects undecodable names: {exc}")
        with open(directory + b"/f\xff.bin", "wb") as handle:
            handle.write(b"x" * 8192)

        status, _, error = self.snapshot("--file-top", "10")
        self.assertEqual(status, 0, error)

        status, output, error = self.run_qdu("show", "--format", "json")
        self.assertEqual(status, 0, error)
        paths = [item["path"] for item in json.loads(output)["entries"]]
        self.assertEqual(paths, ["latin1-\\xe9t\\xe9"])
        for arguments in (("files",), ("files", "--live")):
            status, output, error = self.run_qdu(*arguments, "--format", "json")
            self.assertEqual(status, 0, error)
            self.assertEqual(
                [item["path"] for item in json.loads(output)["files"]],
                ["latin1-\\xe9t\\xe9/f\\xff.bin"],
            )
        status, output, error = self.run_qdu("show", "--style", "plain")
        self.assertEqual(status, 0, error)
        self.assertIn("latin1-\\xe9t\\xe9", output)

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
        self.assertIn(f"holds snapshots of {self.root}, not {other}", error)
        status, output, _ = self.run_qdu("doctor", "--format", "json")
        self.assertEqual(status, 1)
        items = {item["name"]: item for item in json.loads(output)["items"]}
        # The default profile scans the home directory, not the snapshots' root.
        self.assertEqual(items["Snapshot root"]["status"], "FAILED")
        self.assertEqual(items["Latest snapshot"]["status"], "OK")

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
        self.assertGreater(json.loads(output)["entries"][0]["change"], 0)

    def test_listing_failure_midway_is_recorded_not_fatal(self) -> None:
        self.write_file("flaky/a", 1024)
        self.write_file("flaky/b", 1024)
        self.write_file("ok/file", 4096)
        flaky = self.root / "flaky"
        real_scandir = os.scandir

        class BrokenListing:
            """Yield one real entry, then fail like an NFS read error."""

            def __init__(self, path):
                self._inner = real_scandir(path)
                self._yielded = False

            def __iter__(self):
                return self

            def __next__(self):
                if self._yielded:
                    raise OSError(errno.EIO, "Input/output error")
                self._yielded = True
                return next(self._inner)

            def close(self):
                self._inner.close()

            def __enter__(self):
                return self

            def __exit__(self, *exc_info):
                self.close()

        def fake_scandir(path):
            if Path(path) == flaky:
                return BrokenListing(path)
            return real_scandir(path)

        with patch("qdu.scanner.os.scandir", side_effect=fake_scandir):
            status, _, error = self.snapshot()
            live = self.run_qdu(
                "files", "--live", "--path", str(self.root), "--format", "json"
            )
        self.assertEqual(status, 3, error)
        _, output, _ = self.run_qdu("errors", "--format", "json")
        self.assertEqual(
            [
                (item["path"], item["operation"])
                for item in json.loads(output)["errors"]
            ],
            [("flaky", "readdir")],
        )
        _, output, _ = self.run_qdu(
            "show", "--snapshot", "latest-any", "--format", "json"
        )
        self.assertIn("ok", [item["path"] for item in json.loads(output)["entries"]])
        live_status, _, live_error = live
        self.assertEqual(live_status, 0, live_error)
        self.assertIn("flaky", live_error)

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
