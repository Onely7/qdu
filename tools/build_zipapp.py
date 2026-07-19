"""Build the self-contained qdu zipapp release artifact."""

from __future__ import annotations

import shutil
import tempfile
import zipapp
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "src"
DESTINATION = ROOT / "dist" / "qdu"

with tempfile.TemporaryDirectory() as temporary_directory:
    temporary = Path(temporary_directory)
    shutil.copytree(SOURCE / "qdu", temporary / "qdu")
    (temporary / "__main__.py").write_text(
        "from qdu.cli import main\nraise SystemExit(main())\n",
        encoding="utf-8",
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
