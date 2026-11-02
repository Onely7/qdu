"""Build the self-contained qdu zipapp release artifact."""

from __future__ import annotations

import compileall
import py_compile
import shutil
import tempfile
import zipapp
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "src"
DESTINATION = ROOT / "dist" / "qdu"

with tempfile.TemporaryDirectory() as temporary_directory:
    temporary = Path(temporary_directory)
    shutil.copytree(
        SOURCE / "qdu",
        temporary / "qdu",
        ignore=shutil.ignore_patterns("__pycache__"),
    )
    (temporary / "__main__.py").write_text(
        "from qdu.cli import main\nraise SystemExit(main())\n",
        encoding="utf-8",
    )
    # zipimport cannot cache bytecode, so an archive of sources is compiled
    # again on every run. Ship bytecode next to each module for the building
    # interpreter; another Python version ignores it and falls back to the
    # source. Unchecked hash-based files avoid zip's two-second timestamps.
    compileall.compile_dir(
        temporary,
        quiet=1,
        legacy=True,
        invalidation_mode=py_compile.PycInvalidationMode.UNCHECKED_HASH,
    )
    DESTINATION.parent.mkdir(parents=True, exist_ok=True)
    zipapp.create_archive(
        temporary,
        target=DESTINATION,
        interpreter="/usr/bin/env python3",
        compressed=True,
    )
DESTINATION.chmod(0o755)
print(DESTINATION)
