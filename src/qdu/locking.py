"""Coordinate profile mutations with advisory process locks."""

from __future__ import annotations

import fcntl
import json
import os
import socket
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import IO

from qdu.errors import BusyError


@dataclass(slots=True)
class ProfileLock:
    """Hold an exclusive lock and publish diagnostic process metadata."""

    path: Path
    command: str
    handle: IO[str] | None = None

    def __enter__(self) -> ProfileLock:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        handle = self.path.open("a+", encoding="utf-8")
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            handle.seek(0)
            detail = handle.read().strip()
            handle.close()
            message = "another qdu operation is active"
            if detail:
                message += f": {detail}"
            raise BusyError(message) from exc
        metadata = {
            "pid": os.getpid(),
            "host": socket.gethostname(),
            "started_epoch": int(time.time()),
            "command": self.command,
            "argv": sys.argv,
        }
        handle.seek(0)
        handle.truncate()
        json.dump(metadata, handle, ensure_ascii=False, sort_keys=True)
        handle.write("\n")
        handle.flush()
        os.fsync(handle.fileno())
        self.handle = handle
        return self

    def __exit__(self, exc_type: object, exc: object, traceback: object) -> None:
        if self.handle is None:
            return
        self.handle.seek(0)
        self.handle.truncate()
        self.handle.flush()
        fcntl.flock(self.handle.fileno(), fcntl.LOCK_UN)
        self.handle.close()
        self.handle = None


def inspect_lock(path: Path) -> tuple[bool, dict[str, object] | None]:
    """Inspect lock activity and best-effort metadata without taking ownership."""
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    handle = path.open("a+", encoding="utf-8")
    active = False
    try:
        try:
            fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            active = True
        handle.seek(0)
        raw = handle.read().strip()
        metadata = json.loads(raw) if raw else None
        if not active:
            fcntl.flock(handle.fileno(), fcntl.LOCK_UN)
        return active, metadata if isinstance(metadata, dict) else None
    except (json.JSONDecodeError, OSError):
        return active, None
    finally:
        handle.close()


def clear_stale_lock(path: Path) -> bool:
    """Remove an inactive lock file without breaking an active operation."""
    active, _ = inspect_lock(path)
    if active:
        return False
    try:
        path.unlink(missing_ok=True)
    except OSError:
        return False
    return True
