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


