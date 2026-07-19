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


class QduIntegrationTestBase(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.base = Path(self.temporary.name).resolve()
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
