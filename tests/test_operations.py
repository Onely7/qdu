from __future__ import annotations

import json
import os
import sqlite3
from contextlib import closing
from pathlib import Path
from unittest.mock import patch

from qdu import repository
from qdu.locking import ProfileLock
from qdu.storage import (
    IndexRepository,
    ProfilePaths,
    gzip_snapshot,
    snapshot_metadata,
)
from tests.support import QduIntegrationTestBase

# Snapshot schema 2 as released in qdu 0.1.0: directories had a parent column
# and three secondary indexes, and the scanner left a seen_inodes table.
SCHEMA_2_SQL = """
CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE directories (
    path TEXT PRIMARY KEY, parent TEXT NOT NULL, depth INTEGER NOT NULL,
    allocated_bytes INTEGER NOT NULL, apparent_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL, directory_count INTEGER NOT NULL,
    inode_count INTEGER NOT NULL, latest_modified_epoch REAL NOT NULL
) WITHOUT ROWID;
CREATE INDEX directories_allocated_idx ON directories(allocated_bytes DESC, path);
CREATE INDEX directories_inode_idx ON directories(inode_count DESC, path);
CREATE INDEX directories_modified_idx ON directories(latest_modified_epoch, path);
CREATE TABLE owners (
    uid INTEGER PRIMARY KEY, allocated_bytes INTEGER NOT NULL,
    apparent_bytes INTEGER NOT NULL, file_count INTEGER NOT NULL,
    directory_count INTEGER NOT NULL, inode_count INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE owner_directories (
    uid INTEGER NOT NULL, path TEXT NOT NULL, depth INTEGER NOT NULL,
    allocated_bytes INTEGER NOT NULL, apparent_bytes INTEGER NOT NULL,
    file_count INTEGER NOT NULL, directory_count INTEGER NOT NULL,
    inode_count INTEGER NOT NULL, PRIMARY KEY(uid, path)
) WITHOUT ROWID;
CREATE TABLE large_files (
    path TEXT PRIMARY KEY, uid INTEGER NOT NULL, allocated_bytes INTEGER NOT NULL,
    apparent_bytes INTEGER NOT NULL, modified_epoch REAL NOT NULL
) WITHOUT ROWID;
CREATE TABLE scan_errors (
    id INTEGER PRIMARY KEY, path TEXT NOT NULL, operation TEXT NOT NULL,
    message TEXT NOT NULL
);
"""


class OperationsCommandTest(QduIntegrationTestBase):
    def test_rejects_profile_names_that_can_escape_the_state_directory(self) -> None:
        absolute_name = str((self.base / "escape").resolve())
        for profile_name in (
            "../escape",
            absolute_name,
            "nested/name",
            "nested\\name",
        ):
            with self.subTest(profile_name=profile_name):
                status, _, error = self.run_qdu(
                    "profile", "remove", profile_name, "--delete-data"
                )
                self.assertEqual(status, 2)
                self.assertIn("profile names", error)

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

    def test_profile_settings_read_like_the_configuration_file(self) -> None:
        status, _, error = self.run_qdu(
            "profile", "add", "data", "--path", str(self.root),
            "--exclude", "cache/", "--exclude", "*.tmp", "--capacity-limit", "1.5T",
        )  # fmt: skip
        self.assertEqual(status, 0, error)
        config_text = (self.config / "qdu" / "config.ini").read_text(encoding="utf-8")
        self.assertIn("capacity_limit = 1.5TiB", config_text)
        for command in (
            ("profile", "show", "data"),
            ("config", "show", "--profile", "data"),
        ):
            _, output, _ = self.run_qdu(*command)
            lines = output.splitlines()
            self.assertIn("exclude            cache/", lines)
            self.assertIn("                   *.tmp", lines)
            self.assertIn("record_max_depth   unlimited", lines)
            self.assertIn("collect_users      false", lines)
            self.assertIn("capacity_limit     1.5TiB", lines)
            self.assertIn("capacity_user      not set", lines)
            self.assertNotRegex(output, r"None|False|\[")
        _, output, _ = self.run_qdu("profile", "show", "data", "--format", "tsv")
        rows = [line.split("\t") for line in output.splitlines()]
        self.assertEqual(rows[0], ["key", "value"])
        self.assertIn(["exclude", "cache/"], rows)
        self.assertIn(["exclude", "*.tmp"], rows)
        self.assertIn(["capacity_user", ""], rows)

    def test_profile_list_counts_snapshots(self) -> None:
        self.environment["COLUMNS"] = "400"  # keep the temporary root whole
        self.write_file("data", 1024)
        self.snapshot()
        self.snapshot()
        status, output, _ = self.run_qdu("profile", "list", "--style", "plain")
        self.assertEqual(status, 0)
        header, _, row = output.splitlines()
        self.assertEqual(
            header.split(), ["Profile", "Snapshots", "Latest", "Capacity", "Root"]
        )
        self.assertRegex(
            row, r"^default +2 +\d{4}-\d\d-\d\d \d\d:\d\d \(\d+s ago\) +not set +"
        )
        self.assertTrue(row.endswith(str(self.home)), row)
        self.assertFalse(any(line.endswith(" ") for line in output.splitlines()))

    def test_schema_2_snapshots_remain_readable(self) -> None:
        self.write_file("data/file", 1024)
        self.snapshot()
        paths = self.profile_paths()
        current = paths.snapshots / self.index().snapshots[-1].filename
        legacy = current.with_name("legacy.tmp")
        with closing(sqlite3.connect(current)) as source:
            metadata = list(source.execute("SELECT key, value FROM metadata"))
            directories = list(source.execute("SELECT * FROM directories"))
        with closing(sqlite3.connect(legacy)) as target:
            target.executescript(SCHEMA_2_SQL)
            target.executemany(
                "INSERT INTO metadata VALUES (?, ?)",
                [
                    (key, "2" if key == "schema_version" else value)
                    for key, value in metadata
                ],
            )
            target.executemany(
                "INSERT INTO directories VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    (path, path.rsplit("/", 1)[0] if "/" in path else ".", *rest)
                    for path, *rest in directories
                ],
            )
            target.commit()
        legacy.replace(current)
        status, _, error = self.run_qdu("repair")
        self.assertEqual(status, 0, error)

        self.write_file("data/file", 65536)
        self.snapshot()
        status, _, error = self.run_qdu("verify", "--all")
        self.assertEqual(status, 0, error)
        status, output, error = self.run_qdu(
            "show", "--snapshot", "previous", "--under", "data", "--format", "json"
        )
        self.assertEqual(status, 0, error)
        self.assertEqual(json.loads(output)["scope"], "data")
        status, output, error = self.run_qdu("diff", "--format", "json")
        self.assertEqual(status, 0, error)
        self.assertGreater(json.loads(output)["entries"][0]["change"], 0)

    def test_profile_add_keeps_omitted_settings_and_defaults(self) -> None:
        status, _, _ = self.run_qdu("config", "init")
        self.assertEqual(status, 0)
        config_path = self.config / "qdu" / "config.ini"
        config_path.write_text(
            config_path.read_text(encoding="utf-8").replace(
                "keep_snapshots = 52", "keep_snapshots = 7"
            ),
            encoding="utf-8",
        )

        def profile(*options: str) -> dict:
            status, _, error = self.run_qdu(
                "profile", "add", "data", "--path", str(self.root), *options
            )
            self.assertEqual(status, 0, error)
            _, output, _ = self.run_qdu("profile", "show", "data", "--format", "json")
            return json.loads(output)

        created = profile("--with-users")
        self.assertEqual(created["keep_snapshots"], 7)
        self.assertEqual(created["exclude"], [".git", "node_modules"])
        self.assertTrue(created["collect_users"])
        updated = profile("--file-top", "5")
        self.assertTrue(updated["collect_users"])
        self.assertEqual(updated["large_file_limit"], 5)
        self.assertEqual(updated["keep_snapshots"], 7)
        self.assertFalse(profile("--without-users")["collect_users"])
        self.assertEqual(
            profile("--capacity-limit", "1GiB")["capacity_limit_bytes"], 1024**3
        )
        self.assertIsNone(profile("--no-capacity-limit")["capacity_limit_bytes"])
        status, _, error = self.run_qdu(
            "profile", "add", "data", "--path", str(self.root), "--capacity-user", "x"
        )
        self.assertEqual(status, 2)
        self.assertIn("capacity_user", error)
        self.assertNotIn("capacity_user = x", config_path.read_text(encoding="utf-8"))

    def test_profile_list_shows_valid_profiles_next_to_an_invalid_one(self) -> None:
        status, _, error = self.run_qdu(
            "profile", "add", "good", "--path", str(self.root)
        )
        self.assertEqual(status, 0, error)
        config_path = self.config / "qdu" / "config.ini"
        with config_path.open("a", encoding="utf-8") as handle:
            handle.write("\n[profile:bad]\nkeep_snapshots = many\n")
        status, output, error = self.run_qdu("profile", "list", "--style", "plain")
        self.assertEqual(status, 2)
        self.assertIn("good", output)
        self.assertIn(str(self.root), output)
        self.assertIn("(invalid configuration)", output)
        self.assertIn("invalid configuration for profile 'bad'", error)

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
        first = self.index().snapshots[0].snapshot_id
        options = ("--older-than", "0s", "--keep-latest", "1")
        status, output, _ = self.run_qdu("compact", *options, "--dry-run")
        self.assertEqual(output, f"Would compress 1 snapshot(s):\n  {first}\n")
        self.assertFalse(self.index().snapshots[0].archived)
        status, output, _ = self.run_qdu("compact", *options)
        self.assertEqual(status, 0)
        self.assertIn("Compressed 1 snapshot(s)", output)
        index = self.index()
        self.assertTrue(index.snapshots[0].archived)
        status, output, _ = self.run_qdu("compact", *options)
        self.assertTrue(output.startswith("Nothing to compress"), output)
        status, output, _ = self.run_qdu(
            "show", "--snapshot", "previous", "--format", "json"
        )
        self.assertEqual(status, 0)
        self.assertEqual(
            json.loads(output)["snapshot"]["snapshot_id"],
            index.snapshots[0].snapshot_id,
        )

    def test_interrupted_compaction_keeps_the_index_consistent(self) -> None:
        for size in (1024, 2048, 4096):
            self.write_file("data", size)
            self.snapshot()
        real_gzip = repository.gzip_snapshot
        calls = []

        def fail_second_archive(source, destination):
            calls.append(source)
            if len(calls) == 2:
                raise OSError("simulated disk full")
            real_gzip(source, destination)

        with patch.object(repository, "gzip_snapshot", fail_second_archive):
            status, _, error = self.run_qdu(
                "compact", "--older-than", "0s", "--keep-latest", "1"
            )
        self.assertEqual(status, 1)
        self.assertIn("simulated disk full", error)
        status, _, error = self.run_qdu("verify", "--all")
        self.assertEqual(status, 0, error)

        status, _, error = self.run_qdu(
            "compact", "--older-than", "0s", "--keep-latest", "1"
        )
        self.assertEqual(status, 0, error)
        index = self.index()
        self.assertEqual(
            [item.archived for item in index.snapshots], [True, True, False]
        )
        stored = sorted(path.name for path in self.profile_paths().snapshots.iterdir())
        self.assertEqual(stored, sorted(item.filename for item in index.snapshots))
        status, _, error = self.run_qdu("verify", "--all")
        self.assertEqual(status, 0, error)

    def test_repair_removes_an_archive_left_by_an_interrupted_compaction(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        paths = self.profile_paths()
        record = self.index().snapshots[-1]
        source = paths.snapshots / record.filename
        gzip_snapshot(source, source.with_name(f"{source.name}.gz"))

        status, output, _ = self.run_qdu("repair", "--dry-run")
        self.assertEqual(status, 0)
        self.assertIn(f"remove duplicate archive: {record.filename}.gz", output)
        status, _, error = self.run_qdu("repair")
        self.assertEqual(status, 0, error)
        index = self.index()
        self.assertEqual(
            [(item.filename, item.archived) for item in index.snapshots],
            [(record.filename, False)],
        )
        self.assertEqual(
            [path.name for path in paths.snapshots.iterdir()], [record.filename]
        )

    def test_pruning_survives_a_file_that_cannot_be_deleted(self) -> None:
        self.write_file("data", 1024)
        self.snapshot("--keep-snapshots", "1")
        oldest = self.index().snapshots[0].filename
        real_unlink = Path.unlink

        def refuse_snapshot_deletion(path, *args, **kwargs):
            if path.name == oldest:
                raise PermissionError("simulated read-only file")
            return real_unlink(path, *args, **kwargs)

        with patch.object(Path, "unlink", refuse_snapshot_deletion):
            status, _, error = self.snapshot("--keep-snapshots", "1")
        self.assertEqual(status, 0, error)
        self.assertNotIn(oldest, [item.filename for item in self.index().snapshots])
        status, _, error = self.run_qdu("verify", "--all")
        self.assertEqual(status, 0, error)

    def test_snapshots_compress_older_snapshots_automatically(self) -> None:
        self.write_file("data", 1024)
        for _ in range(4):
            status, _, error = self.snapshot("--compact-after", "0s")
            self.assertEqual(status, 0, error)
        index = self.index()
        self.assertEqual(
            [item.archived for item in index.snapshots], [True, True, False, False]
        )
        stored = sorted(path.name for path in self.profile_paths().snapshots.iterdir())
        self.assertEqual(stored, sorted(item.filename for item in index.snapshots))
        status, output, error = self.run_qdu(
            "snapshot", "--path", str(self.root), "--compact-after", "0s"
        )
        self.assertEqual(status, 0, error)
        self.assertIn("compressed 1 older snapshot(s)", output)

        status, _, error = self.snapshot("--compact-after", "off")
        self.assertEqual(status, 0, error)
        self.assertEqual(
            [item.archived for item in self.index().snapshots],
            [True, True, True, False, False, False],
        )
        status, _, error = self.run_qdu("verify", "--all")
        self.assertEqual(status, 0, error)

    def test_retention_defaults_and_invalid_compaction_ages(self) -> None:
        status, output, _ = self.run_qdu("config", "show", "--format", "json")
        self.assertEqual(status, 0)
        settings = json.loads(output)
        self.assertEqual(settings["keep_snapshots"], 52)
        self.assertEqual(settings["compact_after"], "7d")
        with self.assertRaises(SystemExit):
            self.snapshot("--compact-after", "soon")

    def test_archives_expand_privately_and_corrupt_archives_are_reported(
        self,
    ) -> None:
        self.write_file("data/file", 1024)
        self.snapshot()
        self.write_file("data/file", 8192)
        self.snapshot()
        self.run_qdu("compact", "--older-than", "0s", "--keep-latest", "1")
        temporary = self.base / "tmp"
        temporary.mkdir()
        with patch("tempfile.tempdir", str(temporary)):
            status, _, error = self.run_qdu("diff", "--format", "json")
        self.assertEqual(status, 0, error)
        self.assertEqual(list(temporary.iterdir()), [])
        self.assertFalse(any(self.profile_paths().temporary.glob("*")))

        archive = self.profile_paths().snapshots / self.index().snapshots[0].filename
        archive.write_bytes(archive.read_bytes()[:100])
        status, output, error = self.run_qdu("verify", "--all", "--format", "json")
        self.assertEqual(status, 6, error)
        results = json.loads(output)["results"]
        self.assertEqual([item["status"] for item in results], ["FAILED", "OK"])
        status, output, error = self.run_qdu("repair")
        self.assertEqual(status, 0, error)
        self.assertIn("skip invalid snapshot", output)

    def test_schedule_prints_installs_and_removes_crontab_entries(self) -> None:
        crontab_file = self.base / "crontab.txt"
        crontab_file.write_text("0 1 * * * /usr/bin/true\n", encoding="utf-8")
        bin_dir = self.base / "bin"
        bin_dir.mkdir()
        fake = bin_dir / "crontab"
        fake.write_text(
            "#!/bin/sh\n"
            'if [ "$1" = "-l" ]; then\n'
            '  [ -f "$CRONTAB_FILE" ] || { echo "no crontab for test" >&2; exit 1; }\n'
            '  cat "$CRONTAB_FILE"\n'
            'else cat > "$CRONTAB_FILE"; fi\n',
            encoding="utf-8",
        )
        fake.chmod(0o755)
        self.environment["PATH"] = f"{bin_dir}:{os.environ.get('PATH', '')}"
        self.environment["CRONTAB_FILE"] = str(crontab_file)
        executable = str(self.base / "qdu%bin")

        status, output, error = self.run_qdu("schedule", "--qdu", executable)
        self.assertEqual(status, 0, error)
        entry = output.strip().splitlines()[-1]
        self.assertTrue(entry.startswith("15 3 * * 1 XDG_CONFIG_HOME="), entry)
        self.assertIn(" snapshot --profile default --quiet >>", entry)
        self.assertIn("qdu\\%bin", entry)
        self.assertTrue(entry.endswith("2>&1 # qdu:default"))
        self.assertEqual(crontab_file.read_text(encoding="utf-8").count("qdu"), 0)

        for every in ("daily", "weekly"):
            status, _, error = self.run_qdu(
                "schedule",
                "--qdu",
                executable,
                "--every",
                every,
                "--at",
                "04:30",
                "--install",
            )
            self.assertEqual(status, 0, error)
        lines = crontab_file.read_text(encoding="utf-8").splitlines()
        self.assertEqual(lines[0], "0 1 * * * /usr/bin/true")
        self.assertEqual(len(lines), 2)
        self.assertTrue(lines[1].startswith("30 4 * * 1 "))

        def doctor_schedule() -> dict:
            _, output, _ = self.run_qdu("doctor", "--format", "json")
            items = json.loads(output)["items"]
            return next(item for item in items if item["name"] == "Schedule")

        self.assertEqual(
            doctor_schedule(),
            {"name": "Schedule", "status": "OK", "detail": "cron 30 4 * * 1"},
        )
        status, output, error = self.run_qdu("schedule", "--remove")
        self.assertEqual(status, 0, error)
        self.assertEqual(
            crontab_file.read_text(encoding="utf-8"), "0 1 * * * /usr/bin/true\n"
        )
        self.assertEqual(doctor_schedule()["status"], "INFO")
        crontab_file.unlink()
        status, output, _ = self.run_qdu("schedule", "--remove")
        self.assertIn("No scheduled snapshot", output)

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
                _, output, _ = self.run_qdu("doctor", "--format", "json")
        self.assertEqual(status, 5)
        self.assertRegex(
            error,
            r"another qdu operation is using this profile: test holder "
            rf"\(pid {os.getpid()} on .+ since \d{{4}}-\d\d-\d\d \d\d:\d\d\)",
        )
        lock = next(i for i in json.loads(output)["items"] if i["name"] == "Lock")
        self.assertEqual(lock["status"], "BUSY")
        self.assertTrue(lock["detail"].startswith("test holder (pid "), lock)

    def test_growth_check_and_exclusion_difference_warning(self) -> None:
        self.write_file("data/file", 1024)
        self.snapshot("--exclude", ".git")
        self.write_file("data/file", 32768)
        self.snapshot("--exclude", "node_modules")
        status, output, error = self.run_qdu("diff", "--format", "json")
        self.assertEqual(status, 0)
        self.assertIn("exclusion patterns differ", error)
        status, output, _ = self.run_qdu(
            "check", "--under", "data", "--growth-over", "1B", "--format", "json"
        )
        self.assertEqual(status, 10)
        self.assertFalse(json.loads(output)["passed"])

    def test_growth_check_requires_the_path_in_one_snapshot(self) -> None:
        self.write_file("data/file", 1024)
        self.snapshot()
        self.write_file("created/file", 32768)
        self.snapshot()
        status, _, error = self.run_qdu(
            "check", "--under", "no/such/dir", "--growth-over", "1B"
        )
        self.assertEqual(status, 2)
        self.assertIn("absent from both snapshots", error)
        status, output, _ = self.run_qdu(
            "check", "--under", "created", "--growth-over", "1B", "--format", "json"
        )
        self.assertEqual(status, 10)
        self.assertFalse(json.loads(output)["passed"])

    def test_snapshot_age_check_alerts_on_stale_snapshots(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        created = self.index().snapshots[-1].created_epoch
        for days, expected_status in ((1, 0), (10, 10)):
            with (
                self.subTest(days=days),
                patch(
                    "qdu.commands.operations.time.time",
                    return_value=created + days * 86400,
                ),
            ):
                status, output, error = self.run_qdu(
                    "check", "--snapshot-age-over", "8d", "--format", "json"
                )
                self.assertEqual(status, expected_status, error)
                check = json.loads(output)["checks"][0]
                self.assertEqual(check["name"], "snapshot_age")
                self.assertEqual(check["observed"], f"{days}d")
                self.assertEqual(check["threshold"], "8d")
        status, _, error = self.run_qdu("check", "--snapshot-age-over", "soon")
        self.assertEqual(status, 2)
        self.assertIn("invalid duration", error)

    def test_metadata_left_by_a_crashed_process_does_not_block(self) -> None:
        paths = self.profile_paths()
        paths.ensure()
        paths.lock.write_text('{"pid": 999999, "host": "stale"}\n', encoding="utf-8")
        self.write_file("data", 1024)
        status, _, error = self.snapshot()
        self.assertEqual(status, 0, error)
        _, output, _ = self.run_qdu("doctor", "--format", "json")
        lock = next(i for i in json.loads(output)["items"] if i["name"] == "Lock")
        self.assertEqual(lock, {"name": "Lock", "status": "OK", "detail": "not held"})

    def test_repair_dry_run_preserves_index(self) -> None:
        self.write_file("data", 1024)
        self.snapshot()
        paths = self.profile_paths()
        before = paths.index.read_bytes()
        status, output, _ = self.run_qdu("repair", "--dry-run")
        self.assertEqual(status, 0)
        self.assertEqual(paths.index.read_bytes(), before)
        self.assertIn("dry run; nothing was changed", output)
        self.assertNotIn("lock", output)

    def test_malformed_configuration_is_a_usage_error(self) -> None:
        config_path = self.config / "qdu" / "config.ini"
        config_path.parent.mkdir(parents=True)
        config_path.write_text(
            "[defaults]\nkeep_snapshots = not-a-number\n", encoding="utf-8"
        )
        status, _, error = self.run_qdu("doctor")
        self.assertEqual(status, 2)
        self.assertIn("invalid configuration", error)
