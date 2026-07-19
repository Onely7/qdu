"""Define qdu's command-line grammar and process exit behavior."""

from __future__ import annotations

import argparse
import os
import sqlite3
import sys
from collections.abc import Sequence

from qdu._version import __version__
from qdu.commands import (
    browse_command,
    check_command,
    compact_command,
    config_command,
    diff_command,
    doctor_command,
    errors_command,
    explain_command,
    files_command,
    inodes_command,
    list_command,
    profile_command,
    repair_command,
    show_command,
    snapshot_command,
    unlock_command,
    users_command,
    verify_command,
)
from qdu.config import ConfigRepository
from qdu.errors import QduError, ThresholdExceeded
from qdu.staleness import DEFAULT_STALE_THRESHOLDS, StaleThresholds
from qdu.units import parse_size


def positive_int(value: str) -> int:
    """Parse an integer strictly greater than zero for argparse."""
    parsed = int(value)
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def non_negative_int(value: str) -> int:
    """Parse an integer greater than or equal to zero for argparse."""
    parsed = int(value)
    if parsed < 0:
        raise argparse.ArgumentTypeError("must be a non-negative integer")
    return parsed


def percentage(value: str) -> float:
    """Parse an inclusive percentage from zero through one hundred."""
    parsed = float(value)
    if not 0 <= parsed <= 100:
        raise argparse.ArgumentTypeError("must be between 0 and 100")
    return parsed


def capacity_size(value: str) -> int:
    """Parse a strictly positive capacity size for argparse."""
    try:
        parsed = parse_size(value)
    except QduError as exc:
        raise argparse.ArgumentTypeError(str(exc)) from exc
    if parsed <= 0:
        raise argparse.ArgumentTypeError("must be greater than zero")
    return parsed


def stale_thresholds(value: str) -> StaleThresholds:
    """Parse four inactivity thresholds for argparse."""
    try:
        return StaleThresholds.parse(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(str(exc)) from exc


def build_parser() -> argparse.ArgumentParser:
    """Build the complete qdu argument parser without reading process state."""
    parser = argparse.ArgumentParser(
        prog="qdu",
        description="Fast, profile-aware disk-usage snapshots for ordinary users.",
    )
    parser.add_argument("--version", action="version", version=f"qdu {__version__}")
    subparsers = parser.add_subparsers(dest="command")

    snapshot = subparsers.add_parser("snapshot", help="capture a new snapshot")
    _profile_option(snapshot)
    snapshot.add_argument("-p", "--path")
    snapshot.add_argument("--exclude", action="append", default=[])
    snapshot.add_argument("--exclude-from", action="append", default=[])
    snapshot.add_argument(
        "--keep-snapshots", type=non_negative_int, default=argparse.SUPPRESS
    )
    users_group = snapshot.add_mutually_exclusive_group()
    users_group.add_argument(
        "--with-users",
        action="store_true",
        dest="with_users",
        default=argparse.SUPPRESS,
    )
    users_group.add_argument(
        "--without-users",
        action="store_false",
        dest="with_users",
        default=argparse.SUPPRESS,
    )
    snapshot.add_argument(
        "--user-max-depth", type=non_negative_int, default=argparse.SUPPRESS
    )
    snapshot.add_argument(
        "--file-top", type=non_negative_int, default=argparse.SUPPRESS
    )
    snapshot.add_argument(
        "--record-max-depth", type=non_negative_int, default=argparse.SUPPRESS
    )
    filesystem_group = snapshot.add_mutually_exclusive_group()
    filesystem_group.add_argument(
        "--cross-filesystems",
        action="store_true",
        dest="cross_filesystems",
        default=argparse.SUPPRESS,
        help="descend into directories mounted from other filesystems",
    )
    filesystem_group.add_argument(
        "--one-file-system",
        action="store_false",
        dest="cross_filesystems",
        default=argparse.SUPPRESS,
        help="stay on the scan root filesystem (default)",
    )
    snapshot.add_argument("--quiet", action="store_true")
    _display_options(snapshot, include_format=False)

    show = subparsers.add_parser("show", help="show directory capacity ranking")
    _profile_option(show)
    _snapshot_selector(show)
    _ranking_options(show)
    show.add_argument(
        "--metric",
        choices=("allocated_bytes", "apparent_bytes"),
        default="allocated_bytes",
    )
    show.add_argument("--warn-age")
    _staleness_options(show)
    _capacity_options(show)
    show.add_argument("--users", action="store_true")
    show.add_argument("--user-top", type=positive_int, default=10)
    show.add_argument("--user-dir-top", type=non_negative_int, default=5)
    _display_options(show)

    diff = subparsers.add_parser("diff", help="compare snapshots")
    _profile_option(diff)
    diff.add_argument("--from", dest="from_snapshot")
    diff.add_argument("--to", dest="to_snapshot", default="latest")
    diff.add_argument(
        "--previous",
        action="store_true",
        help="compare previous complete snapshot with latest",
    )
    _ranking_options(diff)
    direction = diff.add_mutually_exclusive_group()
    direction.add_argument("--growth-only", action="store_true")
    direction.add_argument("--shrink-only", action="store_true")
    diff.add_argument(
        "--order", choices=("growth", "shrink", "absolute", "current"), default="growth"
    )
    _display_options(diff)

    listing = subparsers.add_parser("list", help="list snapshots")
    _profile_option(listing)
    listing.add_argument("-n", "--top", type=positive_int, default=20)
    _display_options(listing)

    users = subparsers.add_parser("users", help="show owner usage")
    _profile_option(users)
    _snapshot_selector(users)
    users.add_argument("-n", "--top", type=positive_int, default=10)
    users.add_argument("--dirs", type=non_negative_int, default=5)
    users.add_argument(
        "--metric",
        choices=("allocated_bytes", "inode_count"),
        default="allocated_bytes",
    )
    _display_options(users)

    errors = subparsers.add_parser(
        "errors", help="show paths that could not be scanned"
    )
    _profile_option(errors)
    _snapshot_selector(errors)
    errors.set_defaults(snapshot="latest-any")
    errors.add_argument("-n", "--top", type=positive_int, default=100)
    _display_options(errors)

    files = subparsers.add_parser("files", help="show largest files")
    _profile_option(files)
    _snapshot_selector(files)
    files.add_argument(
        "--live", action="store_true", help="scan current filesystem for exact results"
    )
    files.add_argument(
        "--path", help="root for --live; defaults to selected snapshot root"
    )
    files.add_argument("-n", "--top", type=positive_int, default=30)
    files.add_argument("--under", default=".")
    files.add_argument("--min-size", default="0")
    files.add_argument("--user")
    _staleness_options(files)
    _display_options(files)

    inodes = subparsers.add_parser("inodes", help="show inode usage")
    _profile_option(inodes)
    _snapshot_selector(inodes)
    _ranking_options(inodes, include_size=False)
    inodes.add_argument("--users", action="store_true")
    inodes.add_argument("--dirs", type=non_negative_int, default=5)
    _display_options(inodes)

    explain = subparsers.add_parser("explain", help="drill into one directory")
    _profile_option(explain)
    explain.add_argument("path_argument")
    _snapshot_selector(explain)
    explain.add_argument("--diff", action="store_true")
    explain.add_argument("--from", dest="from_snapshot")
    explain.add_argument("--to", dest="to_snapshot", default="latest")
    explain.add_argument("--growth-only", action="store_true")
    explain.add_argument("--shrink-only", action="store_true")
    explain.add_argument(
        "--order", choices=("growth", "shrink", "absolute", "current"), default="growth"
    )
    explain.add_argument("-n", "--top", type=positive_int, default=20)
    explain.add_argument("--min-size", default="0")
    explain.add_argument("--match")
    explain.add_argument(
        "--metric",
        choices=("allocated_bytes", "apparent_bytes"),
        default="allocated_bytes",
    )
    explain.add_argument("--warn-age")
    _staleness_options(explain)
    explain.add_argument("--users", action="store_true")
    explain.add_argument("--user-top", type=positive_int, default=10)
    explain.add_argument("--user-dir-top", type=non_negative_int, default=5)
    _display_options(explain)

    browse = subparsers.add_parser("browse", help="select a directory with fzf")
    _profile_option(browse)
    _snapshot_selector(browse)
    browse.add_argument("-L", "--max-depth", type=positive_int, default=1)
    browse.add_argument("-n", "--top", type=positive_int, default=20)
    browse.add_argument("--min-size", default="0")
    browse.add_argument("--match")
    browse.add_argument(
        "--metric",
        choices=("allocated_bytes", "apparent_bytes"),
        default="allocated_bytes",
    )
    browse.add_argument("--warn-age")
    _staleness_options(browse)
    browse.add_argument("--users", action="store_true")
    browse.add_argument("--user-top", type=positive_int, default=10)
    browse.add_argument("--user-dir-top", type=non_negative_int, default=5)
    _display_options(browse)

    check = subparsers.add_parser(
        "check", help="check growth and filesystem thresholds"
    )
    _profile_option(check)
    _snapshot_selector(check)
    check.add_argument("--path", dest="path_argument", default=".")
    check.add_argument("--growth-over")
    check.add_argument("--disk-usage-over", type=percentage)
    check.add_argument("--inode-usage-over", type=percentage)
    _capacity_options(check)
    _display_options(check)

    compact = subparsers.add_parser("compact", help="gzip old snapshot databases")
    _profile_option(compact)
    compact.add_argument("--older-than", default="30d")
    compact.add_argument("--keep-latest", type=non_negative_int, default=2)
    compact.add_argument("--dry-run", action="store_true")
    _display_options(compact, include_format=False)

    verify = subparsers.add_parser(
        "verify", help="verify snapshot checksums and databases"
    )
    _profile_option(verify)
    _snapshot_selector(verify)
    verify.add_argument("--all", action="store_true")
    _display_options(verify)

    doctor = subparsers.add_parser("doctor", help="inspect the local qdu environment")
    _profile_option(doctor)
    _display_options(doctor)

    repair = subparsers.add_parser(
        "repair", help="rebuild the index and clean stale files"
    )
    _profile_option(repair)
    repair.add_argument("--dry-run", action="store_true")
    _display_options(repair, include_format=False)

    unlock = subparsers.add_parser("unlock", help="remove stale lock metadata")
    _profile_option(unlock)
    _display_options(unlock, include_format=False)

    profiles = subparsers.add_parser("profile", help="manage scan profiles")
    profile_subparsers = profiles.add_subparsers(dest="profile_action", required=True)
    profile_list = profile_subparsers.add_parser("list")
    _display_options(profile_list, include_format=False)
    profile_show = profile_subparsers.add_parser("show")
    profile_show.add_argument("name")
    _display_options(profile_show)
    profile_add = profile_subparsers.add_parser("add")
    profile_add.add_argument("name")
    profile_add.add_argument("--path", required=True)
    profile_add.add_argument("--exclude", action="append", default=[])
    profile_add.add_argument("--keep-snapshots", type=non_negative_int, default=100)
    profile_add.add_argument("--with-users", action="store_true")
    profile_add.add_argument("--user-max-depth", type=non_negative_int, default=3)
    profile_add.add_argument("--file-top", type=non_negative_int, default=1000)
    profile_add.add_argument("--record-max-depth", type=non_negative_int)
    _capacity_options(profile_add)
    profile_filesystem_group = profile_add.add_mutually_exclusive_group()
    profile_filesystem_group.add_argument(
        "--cross-filesystems",
        action="store_true",
        dest="cross_filesystems",
        help="descend into directories mounted from other filesystems",
    )
    profile_filesystem_group.add_argument(
        "--one-file-system",
        action="store_false",
        dest="cross_filesystems",
        help="stay on the profile root filesystem (default)",
    )
    profile_add.set_defaults(cross_filesystems=False)
    _display_options(profile_add, include_format=False)
    profile_remove = profile_subparsers.add_parser("remove")
    profile_remove.add_argument("name")
    profile_remove.add_argument("--delete-data", action="store_true")
    _display_options(profile_remove, include_format=False)

    configuration = subparsers.add_parser(
        "config", help="show or initialize configuration"
    )
    config_subparsers = configuration.add_subparsers(
        dest="config_action", required=True
    )
    config_path = config_subparsers.add_parser("path")
    _display_options(config_path, include_format=False)
    config_init = config_subparsers.add_parser("init")
    _display_options(config_init, include_format=False)
    config_show = config_subparsers.add_parser("show")
    _profile_option(config_show)
    _display_options(config_show)

    return parser


def _profile_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--profile", default=os.environ.get("QDU_PROFILE", "default"))


def _snapshot_selector(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--snapshot", default="latest")


def _ranking_options(
    parser: argparse.ArgumentParser, *, include_size: bool = True
) -> None:
    parser.add_argument("-L", "--max-depth", type=positive_int, default=1)
    parser.add_argument("-n", "--top", type=positive_int, default=20)
    parser.add_argument("--under", default=".")
    if include_size:
        parser.add_argument("--min-size", default="0")
    else:
        parser.set_defaults(min_size="0")
    parser.add_argument("--match")


def _staleness_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--stale",
        action="store_true",
        help="show inactivity age and color stale entries",
    )
    parser.add_argument(
        "--stale-only",
        type=non_negative_int,
        metavar="DAYS",
        help="show only entries whose newest modification is at least DAYS old",
    )
    parser.add_argument(
        "--stale-thresholds",
        type=stale_thresholds,
        default=DEFAULT_STALE_THRESHOLDS,
        metavar="D1,D2,D3,D4",
        help="warning thresholds in days (default: 30,90,180,365)",
    )


def _capacity_options(parser: argparse.ArgumentParser) -> None:
    limit_group = parser.add_mutually_exclusive_group()
    limit_group.add_argument(
        "--capacity-limit",
        type=capacity_size,
        dest="capacity_limit_bytes",
        default=argparse.SUPPRESS,
        metavar="SIZE",
        help="operational capacity limit, for example 2TiB",
    )
    limit_group.add_argument(
        "--no-capacity-limit",
        action="store_const",
        const=None,
        dest="capacity_limit_bytes",
        default=argparse.SUPPRESS,
        help="ignore or clear the configured operational capacity limit",
    )
    target_group = parser.add_mutually_exclusive_group()
    target_group.add_argument(
        "--capacity-user",
        default=argparse.SUPPRESS,
        metavar="USER",
        help="compare the limit with one user's owned allocation",
    )
    target_group.add_argument(
        "--capacity-profile",
        action="store_const",
        const=None,
        dest="capacity_user",
        default=argparse.SUPPRESS,
        help="compare the limit with the whole profile root",
    )


def _display_options(
    parser: argparse.ArgumentParser, *, include_format: bool = True
) -> None:
    if include_format:
        parser.add_argument(
            "--format", choices=("table", "tsv", "json"), default="table"
        )
    else:
        parser.set_defaults(format="table")
    parser.add_argument("--style", choices=("auto", "rich", "plain"), default="auto")
    parser.add_argument("--color", choices=("auto", "always", "never"), default="auto")


def main(argv: Sequence[str] | None = None) -> int:
    """Execute one qdu command and translate expected failures to exit codes.

    Args:
        argv: Arguments excluding the program name. ``None`` reads ``sys.argv``.

    Returns:
        A stable process exit code; no arguments behave as ``qdu show``.
    """
    arguments = list(argv if argv is not None else sys.argv[1:])
    if not arguments:
        arguments = ["show"]
    parser = build_parser()
    args = parser.parse_args(arguments)
    config = ConfigRepository()
    try:
        if args.command == "snapshot":
            return snapshot_command(args, config)
        if args.command == "show":
            return show_command(args, config)
        if args.command == "diff":
            if args.previous:
                args.from_snapshot = None
                args.to_snapshot = "latest"
            return diff_command(args)
        if args.command == "list":
            return list_command(args)
        if args.command == "users":
            return users_command(args)
        if args.command == "errors":
            return errors_command(args)
        if args.command == "files":
            return files_command(args, config)
        if args.command == "inodes":
            return inodes_command(args)
        if args.command == "explain":
            args.under = args.path_argument
            args.max_depth = 1
            return explain_command(args, config)
        if args.command == "browse":
            args.under = "."
            return browse_command(args, config)
        if args.command == "check":
            return check_command(args, config)
        if args.command == "compact":
            return compact_command(args)
        if args.command == "verify":
            return verify_command(args)
        if args.command == "doctor":
            return doctor_command(args, config)
        if args.command == "repair":
            return repair_command(args)
        if args.command == "unlock":
            return unlock_command(args)
        if args.command == "profile":
            return profile_command(args, config)
        if args.command == "config":
            return config_command(args, config)
        parser.error("a command is required")
    except ThresholdExceeded as exc:
        # The report has already been printed. Keep stderr quiet for scheduled checks.
        return exc.exit_code
    except QduError as exc:
        sys.stderr.write(f"qdu: {exc}\n")
        return exc.exit_code
    except (OSError, sqlite3.Error) as exc:
        sys.stderr.write(f"qdu: {exc}\n")
        return 1
    except KeyboardInterrupt:
        sys.stderr.write("qdu: interrupted\n")
        return 130
    return 0
