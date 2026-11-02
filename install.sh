#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
INSTALL_DIR=${QDU_INSTALL_DIR:-"$HOME/.local/bin"}

python3 "$PROJECT_DIR/tools/build_zipapp.py" >/dev/null
mkdir -p "$INSTALL_DIR"
install -m 755 "$PROJECT_DIR/dist/qdu" "$INSTALL_DIR/qdu"
printf 'Installed qdu to %s\n' "$INSTALL_DIR/qdu"
printf 'Make sure %s is included in PATH.\n' "$INSTALL_DIR"
