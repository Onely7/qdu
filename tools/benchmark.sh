#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
QDU=${QDU:-"$PROJECT_DIR/dist/qdu"}
WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT
mkdir -p "$WORK_DIR/home" "$WORK_DIR/state" "$WORK_DIR/config" "$WORK_DIR/root"

python3 - "$WORK_DIR/root" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
for group in range(200):
    directory = root / f"group-{group:04d}"
    directory.mkdir()
    for item in range(50):
        (directory / f"file-{item:04d}.dat").write_bytes(b"x" * ((item % 8 + 1) * 128))
PY

export HOME="$WORK_DIR/home"
export XDG_STATE_HOME="$WORK_DIR/state"
export XDG_CONFIG_HOME="$WORK_DIR/config"

printf 'Dataset: 200 directories, 10,000 files\n'
/usr/bin/time -f 'snapshot elapsed=%e sec max_rss=%M KiB' "$QDU" snapshot --path "$WORK_DIR/root" --with-users --file-top 1000 --quiet
/usr/bin/time -f 'show elapsed=%e sec max_rss=%M KiB' "$QDU" show --max-depth 2 --top 50 --format json >/dev/null
python3 - "$WORK_DIR/root" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
for group in range(0, 200, 4):
    path = root / f"group-{group:04d}" / "growth.dat"
    path.write_bytes(b"y" * 65536)
PY
/usr/bin/time -f 'second_snapshot elapsed=%e sec max_rss=%M KiB' "$QDU" snapshot --path "$WORK_DIR/root" --with-users --file-top 1000 --quiet
/usr/bin/time -f 'diff elapsed=%e sec max_rss=%M KiB' "$QDU" diff --max-depth 2 --top 50 --format json >/dev/null
