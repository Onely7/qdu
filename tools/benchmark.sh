#!/usr/bin/env bash
# Measure elapsed time and peak memory of qdu commands on a generated tree.
#
# QDU selects the executable (default: dist/qdu). QDU_BENCH_DIRECTORIES and
# QDU_BENCH_FILES size the tree (default: 2,000 directories, 200,000 files).
set -euo pipefail

PROJECT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
QDU=${QDU:-"$PROJECT_DIR/dist/qdu"}
DIRECTORIES=${QDU_BENCH_DIRECTORIES:-2000}
FILES=${QDU_BENCH_FILES:-200000}
WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT
mkdir -p "$WORK_DIR/home" "$WORK_DIR/state" "$WORK_DIR/config" "$WORK_DIR/root"

# A random recursive tree: each directory hangs below an earlier one, and most
# files are empty while some hold a few KiB, as in a typical home directory.
python3 - "$WORK_DIR/root" "$DIRECTORIES" "$FILES" <<'PY'
import random
import sys
from pathlib import Path

root = Path(sys.argv[1])
directory_count = int(sys.argv[2])
file_count = int(sys.argv[3])
rng = random.Random(1)
directories = [root]
for index in range(directory_count):
    directory = rng.choice(directories) / f"dir-{index:05d}"
    directory.mkdir()
    directories.append(directory)
payload = b"x" * 8192
for index in range(file_count):
    path = rng.choice(directories) / f"file-{index:07d}.dat"
    path.write_bytes(payload[: rng.randrange(1, 8192)] if rng.random() < 0.15 else b"")
PY

export HOME="$WORK_DIR/home"
export XDG_STATE_HOME="$WORK_DIR/state"
export XDG_CONFIG_HOME="$WORK_DIR/config"
SNAPSHOTS="$XDG_STATE_HOME/qdu/profiles/default/snapshots"

measure() {
  local label=$1
  shift
  /usr/bin/time -f "$label elapsed=%e sec max_rss=%M KiB" "$@"
}

printf 'Dataset: %s directories, %s files\n' "$DIRECTORIES" "$FILES"
measure snapshot "$QDU" snapshot --path "$WORK_DIR/root" --with-users --quiet
printf 'snapshot_file=%s KiB\n' "$(du -k "$SNAPSHOTS"/*.sqlite3 | cut -f1)"
measure show "$QDU" show --max-depth 2 --top 50 --format json >/dev/null
measure show_under "$QDU" show --under dir-00000 --max-depth 3 --top 50 --format json >/dev/null
measure files "$QDU" files --top 50 --format json >/dev/null
python3 - "$WORK_DIR/root" <<'PY'
import sys
from pathlib import Path

for index, directory in enumerate(sorted(Path(sys.argv[1]).rglob("dir-*"))):
    if index % 40 == 0:
        (directory / "growth.dat").write_bytes(b"y" * 65536)
PY
measure second_snapshot "$QDU" snapshot --path "$WORK_DIR/root" --with-users --quiet
measure diff "$QDU" diff --max-depth 2 --top 50 --format json >/dev/null
