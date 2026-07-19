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
