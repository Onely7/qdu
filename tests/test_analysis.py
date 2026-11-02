from __future__ import annotations

import json
import os
import time

from tests.support import QduIntegrationTestBase


class AnalysisCommandTest(QduIntegrationTestBase):
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
        options = ("--capacity-limit", "1GiB", "--capacity-user", str(os.getuid()))
        # 'show' still ranks directories; 'check' needs a definite answer.
        status, output, error = self.run_qdu("show", *options, "--format", "json")
        self.assertEqual(status, 0, error)
        self.assertIsNone(json.loads(output)["capacity"])
        self.assertIn("capacity limit not evaluated", error)
        self.assertIn("--with-users", error)
        status, _, error = self.run_qdu("check", *options)
        self.assertEqual(status, 2)
        self.assertIn("--with-users", error)

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

    def test_show_users_requires_owner_statistics_in_every_format(self) -> None:
        self.write_file("alice/data.bin", 8192)
        self.snapshot()
        status, _, error = self.run_qdu("show", "--users")
        self.assertEqual(status, 2)
        self.assertIn("--with-users", error)

        self.snapshot("--with-users")
        status, output, error = self.run_qdu("show", "--users", "--format", "json")
        self.assertEqual(status, 0, error)
        users = json.loads(output)["users"]
        self.assertEqual(users[0]["uid"], os.getuid())
        self.assertIn("directories", users[0])
        status, output, _ = self.run_qdu("show", "--format", "json")
        self.assertNotIn("users", json.loads(output))
        status, _, error = self.run_qdu("show", "--users", "--format", "tsv")
        self.assertEqual(status, 2)
        self.assertIn("qdu users --format tsv", error)

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

    def test_live_and_snapshot_files_count_sparse_allocation_alike(self) -> None:
        self.write_file("dense.bin", 8192)
        with (self.root / "sparse.bin").open("wb") as handle:
            handle.truncate(1 << 20)
        self.snapshot("--file-top", "10")

        def sizes(*options: str) -> dict[str, tuple[int, int]]:
            status, output, error = self.run_qdu("files", *options, "--format", "json")
            self.assertEqual(status, 0, error)
            return {
                item["path"]: (item["allocated_bytes"], item["apparent_bytes"])
                for item in json.loads(output)["files"]
            }

        snapshot_sizes = sizes()
        self.assertEqual(snapshot_sizes["sparse.bin"][1], 1 << 20)
        self.assertEqual(sizes("--live"), snapshot_sizes)

    def test_files_and_inodes_describe_their_snapshot(self) -> None:
        self.write_file("a/data", 1024)
        self.snapshot("--file-top", "5")
        _, output, _ = self.run_qdu("files", "--style", "plain")
        self.assertRegex(output, r"\nSnapshot +\d{4}-\d\d-\d\d \d\d:\d\d \(")
        self.assertIn("Candidates  the 5 largest files at snapshot time", output)
        _, output, _ = self.run_qdu("files", "--live", "--style", "plain")
        self.assertIn("Source  live scan at ", output)
        _, output, _ = self.run_qdu("inodes", "--style", "plain")
        self.assertIn(f"Root      {self.root}\n", output)
        self.assertRegex(output, r"\nScope +\. +3 inodes\n")
        self.assertRegex(output, r"\nInodes +[0-9.]+% used, [0-9,]+ available of ")

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

    def test_explain_rejects_opposite_change_filters(self) -> None:
        with self.assertRaises(SystemExit) as raised:
            self.run_qdu("explain", "a", "--diff", "--growth-only", "--shrink-only")
        self.assertEqual(raised.exception.code, 2)

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
        self.assertIn("No matching entries.", output)
        status, output, _ = self.run_qdu(
            "show", "--min-size", "1TiB", "--format", "json"
        )
        self.assertEqual(json.loads(output)["entries"], [])

    def test_browse_streams_paths_to_optional_fzf(self) -> None:
        self.write_file("a/child/file", 1024)
        self.write_file(" spaced\nname/file", 4096)
        self.snapshot()
        received = self.base / "fzf-input"
        bin_dir = self.base / "bin"
        bin_dir.mkdir()
        fzf = bin_dir / "fzf"
        fzf.write_text(
            '#!/usr/bin/env sh\ncat >"$FZF_INPUT"\nprintf \'%s\\0\' "$FZF_PICK"\n',
            encoding="utf-8",
        )
        fzf.chmod(0o755)
        self.environment["PATH"] = f"{bin_dir}:{os.environ.get('PATH', '')}"
        self.environment["FZF_INPUT"] = str(received)
        for pick in ("a", " spaced\nname", "a\twith\ttabs"):
            with self.subTest(pick=pick):
                if "\t" in pick:
                    self.write_file(f"{pick}/file", 16384)
                    self.snapshot()
                self.environment["FZF_PICK"] = f"  4.0KiB\t{pick}"
                status, output, error = self.run_qdu("browse", "--format", "json")
                self.assertEqual(status, 0, error)
                self.assertEqual(json.loads(output)["scope"], pick)
        records = received.read_text(encoding="utf-8").split("\0")[:-1]
        size, root = records[0].split("\t", 1)
        self.assertEqual(root, ".")
        self.assertEqual(len(size), 8)
        self.assertTrue(size.endswith("B"))
        paths = [record.split("\t", 1)[1] for record in records]
        self.assertEqual(paths[1], "a\twith\ttabs")
        self.assertIn(" spaced\nname", paths)
