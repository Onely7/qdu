"""Command for taking snapshots periodically through the user's crontab."""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import sys
from argparse import Namespace
from pathlib import Path

from qdu.commands.context import build_renderer
from qdu.config import ConfigRepository
from qdu.errors import UsageError
from qdu.storage import ProfilePaths

WEEKDAYS = ("sun", "mon", "tue", "wed", "thu", "fri", "sat")


def schedule_command(args: Namespace, config: ConfigRepository) -> int:
    """Print, install, or remove the crontab entry for a profile's snapshots.

    Raises:
        UsageError: If the profile is invalid, crontab is unavailable, or the
            qdu executable cannot be located.
    """
    renderer = build_renderer(args)
    config.load_profile(args.profile)
    marker = f"# qdu:{args.profile}"
    if args.remove:
        lines = _read_crontab()
        kept = [line for line in lines if not line.rstrip().endswith(marker)]
        if len(kept) == len(lines):
            renderer.message(f"No scheduled snapshot for profile {args.profile}.")
        else:
            _write_crontab(kept)
            renderer.message(f"Removed the scheduled snapshot for {args.profile}.")
        return 0

    paths = ProfilePaths.for_profile(args.profile)
    entry = f"{_cron_time(args)} {_job(args, paths.root / 'snapshot.log')} {marker}"
    if not args.install:
        renderer.message("Add this line with 'crontab -e', or rerun with --install:")
        renderer.message("")
        renderer.message(entry)
        return 0
    paths.ensure()
    lines = [line for line in _read_crontab() if not line.rstrip().endswith(marker)]
    _write_crontab([*lines, entry])
    renderer.message(f"Installed in your crontab:\n{entry}")
    return 0


def scheduled_entry(profile: str) -> str | None:
    """Return the crontab line that takes the profile's snapshots, if any.

    Raises:
        UsageError: If crontab is unavailable or cannot be read.
    """
    marker = f"# qdu:{profile}"
    return next(
        (line for line in _read_crontab() if line.rstrip().endswith(marker)), None
    )


def _cron_time(args: Namespace) -> str:
    hour, minute = args.at
    if args.every == "daily":
        return f"{minute} {hour} * * *"
    if args.every == "monthly":
        return f"{minute} {hour} 1 * *"
    return f"{minute} {hour} * * {WEEKDAYS.index(args.weekday)}"


def _job(args: Namespace, log: Path) -> str:
    # cron starts jobs with a minimal environment, so carry over the XDG
    # locations that decide where qdu reads its configuration and state.
    assignments = [
        f"{name}={shlex.quote(os.environ[name])}"
        for name in ("XDG_CONFIG_HOME", "XDG_STATE_HOME")
        if os.environ.get(name)
    ]
    executable = [args.qdu] if args.qdu else _running_executable()
    command = [*executable, "snapshot", "--profile", args.profile, "--quiet"]
    words = [
        *assignments,
        *(shlex.quote(part) for part in command),
        f">>{shlex.quote(str(log))}",
        "2>&1",
    ]
    line = " ".join(words)
    # cron turns an unescaped % into a newline.
    return line.replace("%", "\\%")


def _running_executable() -> list[str]:
    program = Path(sys.argv[0]).resolve()
    # A zipapp runs this module from inside its own archive file.
    zipapp = Path(__file__).resolve().is_relative_to(program)
    if program.is_file() and (zipapp or program.name == "qdu"):
        return [str(program)]
    raise UsageError(
        "cannot tell where qdu is installed; install it (for example with "
        "'make install') or pass --qdu PATH"
    )


def _crontab() -> str:
    path = shutil.which("crontab")
    if path is None:
        raise UsageError(
            "crontab is not available; see docs/QDU_TMUX_WEEKLY_SNAPSHOT_GUIDE.md"
        )
    return path


def _read_crontab() -> list[str]:
    # The executable is resolved to an absolute path and no shell is involved.
    result = subprocess.run(  # noqa: S603
        [_crontab(), "-l"], capture_output=True, text=True, check=False
    )
    if result.returncode == 0:
        return result.stdout.splitlines()
    if "no crontab" in result.stderr.lower():
        return []
    raise UsageError(f"cannot read your crontab: {result.stderr.strip()}")


def _write_crontab(lines: list[str]) -> None:
    content = "".join(f"{line}\n" for line in lines)
    result = subprocess.run(  # noqa: S603
        [_crontab(), "-"], input=content, capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise UsageError(f"cannot update your crontab: {result.stderr.strip()}")
