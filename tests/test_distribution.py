from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from qdu import __version__


class DistributionArtifactTest(unittest.TestCase):
    def test_release_manifest_matches_the_runtime_version(self) -> None:
        project = Path(__file__).resolve().parents[1]
        manifest = json.loads(
            (project / ".release-please-manifest.json").read_text(encoding="utf-8")
        )
        config = json.loads(
            (project / "release-please-config.json").read_text(encoding="utf-8")
        )

        self.assertEqual(manifest["."], __version__)
        self.assertEqual(config["release-type"], "python")
        self.assertFalse(config["include-component-in-tag"])
        self.assertTrue(config["include-v-in-tag"])
        self.assertIn(
            {
                "type": "generic",
                "path": "src/qdu/_version.py",
            },
            config["packages"]["."]["extra-files"],
        )
        version_source = (project / "src" / "qdu" / "_version.py").read_text(
            encoding="utf-8"
        )
        self.assertIn("x-release-please-version", version_source)

    def test_zipapp_is_the_tested_release_artifact(self) -> None:
        project = Path(__file__).resolve().parents[1]
        subprocess.run(
            [sys.executable, str(project / "tools" / "build_zipapp.py")], check=True
        )
        artifact = project / "dist" / "qdu"
        result = subprocess.run(
            [str(artifact), "--version"], text=True, capture_output=True, check=True
        )
        self.assertIn(f"qdu {__version__}", result.stdout)
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
