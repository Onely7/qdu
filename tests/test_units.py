from __future__ import annotations

import io
import os
import unittest
from unittest.mock import patch

from qdu.capacity import assess_capacity
from qdu.errors import UsageError
from qdu.patterns import PathPatternMatcher
from qdu.render import ProgressLine, display_safe, display_width, truncate
from qdu.staleness import StaleLevel, StaleThresholds, assess_staleness
from qdu.units import (
    format_age,
    format_bytes,
    format_exact_size,
    parse_duration,
    parse_size,
)

# (pattern, path, the path is a directory, excluded)
PATTERN_CONTRACT = (
    ("cache", "cache", True, True),
    ("cache", "a/cache/x", False, True),
    ("cache", "cached", False, False),
    ("*.tmp", "w/x.tmp", False, True),
    ("*.tmp", "x.tmp.gz", False, False),
    ("/build", "build", True, True),
    ("/build", "build/x", False, True),
    ("/build", "src/build", True, False),
    ("./build", "build", True, True),
    ("./build", "x/build", True, False),
    ("data/raw", "data/raw", True, True),
    ("data/raw", "p/data/raw", True, False),
    ("models/*.bin", "models/m.bin", False, True),
    ("models/*.bin", "run/models/m.bin", False, False),
    ("models/*.bin", "models/sub/m.bin", False, False),
    ("**/models/*.bin", "run/models/m.bin", False, True),
    ("**/models/*.bin", "models/m.bin", False, True),
    ("cache/**", "cache/x", False, True),
    ("cache/**", "cache", True, False),
    ("cache/**", "a/cache/x", False, False),
    ("a/**/c", "a/c", False, True),
    ("a/**/c", "a/b/c", False, True),
    ("a/**/c", "a/b/d/c", False, True),
    ("a/**/c", "x/a/b/c", False, False),
    ("logs/", "logs", True, True),
    ("logs/", "logs", False, False),
    ("logs/", "a/logs/x", False, True),
    ("[ab]?", "ab", False, True),
    ("[!a]x", "bx", False, True),
    ("[!a]x", "ax", False, False),
    ("\\*.txt", "*.txt", False, True),
    ("\\*.txt", "a.txt", False, False),
    ("Cache", "cache", True, False),
)


class UtilityTest(unittest.TestCase):
    def test_units(self) -> None:
        self.assertEqual(parse_size("1KiB"), 1024)
        self.assertEqual(parse_size("1GB"), 1000**3)
        self.assertEqual(parse_size("2T"), 2 * 1024**4)
        self.assertEqual(parse_size("1.5g"), 3 * 512 * 1024**2)
        self.assertEqual(parse_size("1.1KB"), 1100)
        self.assertEqual(parse_size("10"), 10)
        self.assertEqual(parse_size("10b"), 10)
        for invalid in ("10ib", "10 GiB", "GiB", "-1KiB", "1.KiB"):
            with self.subTest(invalid=invalid), self.assertRaises(UsageError):
                parse_size(invalid)
        self.assertEqual(parse_duration("2h"), 7200)
        self.assertEqual(format_bytes(1536), "1.5KiB")
        for seconds, text in ((0, "0s"), (61, "1m1s"), (3601, "1h"), (90061, "1d1h")):
            self.assertEqual(format_age(seconds), text)
        for size, text in ((0, "0B"), (2 * 10**12, "2TB"), (3 * 2**39, "1.5TiB")):
            self.assertEqual(format_exact_size(size), text)
        for size in (1, 999, 1536, 1100, 5 * 2**20 + 2**19, 2**63 - 1):
            self.assertEqual(parse_size(format_exact_size(size)), size)

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

    def test_patterns_follow_the_gitignore_contract(self) -> None:
        for pattern, path, is_directory, excluded in PATTERN_CONTRACT:
            with self.subTest(pattern=pattern, path=path, is_directory=is_directory):
                matcher = PathPatternMatcher((pattern,))
                self.assertEqual(
                    matcher.matches(path, is_directory=is_directory), excluded
                )

    def test_unsupported_patterns_are_rejected(self) -> None:
        for pattern in ("!keep", "/", "./"):
            with self.subTest(pattern=pattern), self.assertRaises(UsageError):
                PathPatternMatcher((pattern,))
        self.assertTrue(PathPatternMatcher(("\\!keep",)).matches("!keep"))

    def test_unicode_width_and_control_escaping(self) -> None:
        self.assertEqual(display_width("abc"), 3)
        self.assertEqual(display_width("日本"), 4)
        self.assertLessEqual(display_width(truncate("日本語ファイル", 6)), 6)
        self.assertEqual(display_safe("a\tb\nc"), "a\\tb\\nc")

    def test_progress_line_redraws_in_place_and_keeps_the_path_tail(self) -> None:
        stream = io.StringIO()
        line = ProgressLine(stream)
        with patch(
            "qdu.render.shutil.get_terminal_size",
            return_value=os.terminal_size((41, 24)),
        ):
            line.update("Scanning 12 entries", "very/long/path/to/the/current/dir")
        written = stream.getvalue()
        self.assertTrue(written.startswith("\r"))
        self.assertTrue(written.endswith("\033[K"))
        shown = written[1 : -len("\033[K")]
        self.assertEqual(display_width(shown), 40)
        self.assertTrue(shown.startswith("Scanning 12 entries: …"))
        self.assertTrue(shown.endswith("current/dir"))
        line.clear()
        self.assertTrue(stream.getvalue().endswith("\r\033[K"))
        line.clear()
        self.assertEqual(stream.getvalue().count("\r\033[K"), 1)
