"""Define qdu's command-line grammar and process exit behavior."""

from __future__ import annotations

import argparse
import os
import sqlite3
import sys
from collections.abc import Sequence
from pathlib import Path

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
    schedule_command,
    show_command,
    snapshot_command,
    users_command,
    verify_command,
)
from qdu.commands.schedule import WEEKDAYS
from qdu.config import ConfigRepository, parse_compact_after
from qdu.errors import QduError, ThresholdExceeded
from qdu.staleness import DEFAULT_STALE_THRESHOLDS, StaleThresholds
from qdu.units import parse_size


def _expanded_path(value: str) -> Path:
    return Path(value).expanduser()


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
        description="Record disk-usage snapshots and explore where space goes. "
        "Without a command, qdu runs 'qdu show'.",
    )
    parser.add_argument("--version", action="version", version=f"qdu {__version__}")
    subparsers = parser.add_subparsers(dest="command", metavar="COMMAND")

    snapshot = _command(
        subparsers, "snapshot", "scan the profile root and save a snapshot"
    )
    _profile_option(snapshot)
    snapshot.add_argument(
        "-p",
        "--path",
        dest="root",
        type=_expanded_path,
        default=argparse.SUPPRESS,
        help="directory to scan (default: the profile's path, or your home directory)",
    )
    snapshot.add_argument(
        "--exclude",
        action="append",
        default=[],
        metavar="PATTERN",
        help="also exclude PATTERN in this run; repeatable",
    )
    snapshot.add_argument(
        "--exclude-from",
        action="append",
        default=[],
        metavar="FILE",
        help="also exclude the patterns in FILE, one per line; repeatable",
    )
    _retention_options(snapshot)
    _users_options(snapshot)
    snapshot.add_argument(
        "--user-max-depth",
        type=non_negative_int,
        default=argparse.SUPPRESS,
        metavar="N",
        help="record owner totals for directories down to depth N (default: 3)",
    )
    snapshot.add_argument(
        "--file-top",
        dest="large_file_limit",
        type=non_negative_int,
        default=argparse.SUPPRESS,
        metavar="N",
        help="remember the N largest files for 'qdu files' (default: 1000)",
    )
    snapshot.add_argument(
        "--record-max-depth",
        type=non_negative_int,
        default=argparse.SUPPRESS,
        metavar="N",
        help="store directories down to depth N only; deeper files still count",
    )
    _filesystem_options(snapshot)
    snapshot.add_argument(
        "--quiet",
        action="store_true",
        help="print nothing on success, not even progress",
    )
    _display_options(snapshot, include_format=False)

    show = _command(subparsers, "show", "rank the largest directories in a snapshot")
    _profile_option(show)
    _snapshot_selector(show)
    _ranking_options(
        show, min_size_help="hide directories smaller than SIZE, such as 1GiB"
    )
    _metric_option(show, "allocated_bytes", "apparent_bytes", purpose="rank by")
    _warn_age_option(show)
    _staleness_options(show)
    _capacity_options(show)
    _owner_report_options(show)
    _display_options(show)

    diff = _command(subparsers, "diff", "rank changes between two snapshots")
    _profile_option(diff)
    _comparison_options(diff)
    _ranking_options(
        diff,
        min_size_help="hide rows whose larger value is below SIZE, such as 1GiB; "
        "with --metric inodes, an inode count",
    )
    _metric_option(
        diff,
        "allocated_bytes",
        "apparent_bytes",
        "inode_count",
        purpose="compare by",
    )
    _display_options(diff)

    listing = _command(subparsers, "list", "list snapshots and their disk use")
    _profile_option(listing)
    listing.add_argument(
        "-n",
        "--top",
        type=positive_int,
        default=20,
        metavar="N",
        help="show the N newest snapshots (default: 20)",
    )
    _display_options(listing)

    users = _command(subparsers, "users", "rank file owners (needs --with-users)")
    _profile_option(users)
    _snapshot_selector(users)
    users.add_argument(
        "-n",
        "--top",
        type=positive_int,
        default=10,
        metavar="N",
        help="show N owners (default: 10)",
    )
    users.add_argument(
        "--dirs",
        type=non_negative_int,
        default=5,
        metavar="N",
        help="show each owner's N largest directories (default: 5)",
    )
    _metric_option(users, "allocated_bytes", "inode_count", purpose="rank owners by")
    _display_options(users)

    errors = _command(subparsers, "errors", "list paths a snapshot could not read")
    _profile_option(errors)
    _snapshot_selector(errors, default="latest-any")
    errors.add_argument(
        "-n",
        "--top",
        type=positive_int,
        default=100,
        metavar="N",
        help="show N errors (default: 100)",
    )
    _display_options(errors)

    files = _command(subparsers, "files", "rank the largest files")
    _profile_option(files)
    _snapshot_selector(files)
    files.add_argument(
        "--live",
        action="store_true",
        help="scan the filesystem now instead of reading the snapshot's candidates",
    )
    files.add_argument(
        "--path",
        metavar="DIR",
        help="directory to scan with --live (default: the snapshot or profile root)",
    )
    files.add_argument(
        "-n",
        "--top",
        type=positive_int,
        default=30,
        metavar="N",
        help="show N files (default: 30)",
    )
    _under_option(files)
    _min_size_option(files, "hide files smaller than SIZE, such as 1GiB")
    files.add_argument(
        "--user",
        metavar="USER",
        help="only files owned by USER, a name or a numeric UID",
    )
    _staleness_options(files)
    _display_options(files)

    inodes = _command(subparsers, "inodes", "rank directories or owners by inodes")
    _profile_option(inodes)
    _snapshot_selector(inodes)
    _ranking_options(inodes, min_size_help=None)
    inodes.add_argument(
        "--users",
        action="store_true",
        help="rank owners instead of directories (needs --with-users)",
    )
    inodes.add_argument(
        "--dirs",
        type=non_negative_int,
        default=5,
        metavar="N",
        help="with --users, show each owner's N largest directories (default: 5)",
    )
    _display_options(inodes)

    explain = _command(subparsers, "explain", "show what is inside one directory")
    _profile_option(explain)
    explain.add_argument(
        "path_argument",
        metavar="PATH",
        help="directory to explain, relative to the snapshot root",
    )
    _snapshot_selector(explain)
    explain.add_argument(
        "--diff",
        action="store_true",
        help="show changes between two snapshots instead of current usage",
    )
    _comparison_options(explain)
    _top_option(explain)
    _min_size_option(explain, "hide directories smaller than SIZE, such as 1GiB")
    _match_option(explain)
    _metric_option(
        explain, "allocated_bytes", "apparent_bytes", purpose="rank or compare by"
    )
    _warn_age_option(explain)
    _staleness_options(explain)
    _owner_report_options(explain)
    _display_options(explain)

    browse = _command(
        subparsers, "browse", "pick a directory with fzf, then show it like 'show'"
    )
    _profile_option(browse)
    _snapshot_selector(browse)
    _max_depth_option(browse)
    _top_option(browse)
    _min_size_option(browse, "hide directories smaller than SIZE, such as 1GiB")
    _match_option(browse)
    _metric_option(browse, "allocated_bytes", "apparent_bytes", purpose="rank by")
    _warn_age_option(browse)
    _staleness_options(browse)
    _owner_report_options(browse)
    _display_options(browse)

    check = _command(
        subparsers, "check", "evaluate alert thresholds; exit status 10 on an alert"
    )
    _profile_option(check)
    _snapshot_selector(check)
    _under_option(check, "directory for --growth-over (default: the root)")
    check.add_argument(
        "--growth-over",
        metavar="SIZE",
        help="alert when the directory grew by more than SIZE since the previous "
        "snapshot",
    )
    check.add_argument(
        "--disk-usage-over",
        type=percentage,
        metavar="PERCENT",
        help="alert when the root's filesystem is fuller than PERCENT now",
    )
    check.add_argument(
        "--inode-usage-over",
        type=percentage,
        metavar="PERCENT",
        help="alert when the root's filesystem uses more than PERCENT of its inodes",
    )
    check.add_argument(
        "--snapshot-age-over",
        metavar="DURATION",
        help="alert when the checked snapshot is older than DURATION, such as 8d",
    )
    _capacity_options(check)
    _display_options(check)

    schedule = _command(
        subparsers, "schedule", "take snapshots periodically with your crontab"
    )
    _profile_option(schedule)
    schedule.add_argument(
        "--every",
        choices=("daily", "weekly", "monthly"),
        default="weekly",
        help="how often to take a snapshot; monthly runs on the 1st (default: weekly)",
    )
    schedule.add_argument(
        "--at",
        type=clock_time,
        default=(3, 15),
        metavar="HH:MM",
        help="local time of day (default: 03:15)",
    )
    schedule.add_argument(
        "--weekday",
        choices=WEEKDAYS,
        default="mon",
        help="day of weekly snapshots (default: mon)",
    )
    schedule.add_argument(
        "--qdu",
        metavar="PATH",
        help="qdu executable for cron to run (default: the one running now)",
    )
    schedule_action = schedule.add_mutually_exclusive_group()
    schedule_action.add_argument(
        "--install",
        action="store_true",
        help="add the entry to your crontab, replacing this profile's old entry",
    )
    schedule_action.add_argument(
        "--remove",
        action="store_true",
        help="remove this profile's entry from your crontab",
    )
    _display_options(schedule, include_format=False)

    compact = _command(subparsers, "compact", "gzip older snapshots now")
    _profile_option(compact)
    compact.add_argument(
        "--older-than",
        default="7d",
        metavar="DURATION",
        help="compress snapshots older than DURATION (default: 7d)",
    )
    compact.add_argument(
        "--keep-latest",
        type=non_negative_int,
        default=2,
        metavar="N",
        help="leave the N newest snapshots uncompressed (default: 2)",
    )
    compact.add_argument(
        "--dry-run", action="store_true", help="list what would be compressed"
    )
    _display_options(compact, include_format=False)

    verify = _command(
        subparsers, "verify", "check snapshot checksums and database integrity"
    )
    _profile_option(verify)
    _snapshot_selector(verify)
    verify.add_argument(
        "--all", action="store_true", help="verify every snapshot of the profile"
    )
    _display_options(verify)

    doctor = _command(subparsers, "doctor", "diagnose the qdu setup of a profile")
    _profile_option(doctor)
    _display_options(doctor)

    repair = _command(
        subparsers, "repair", "rebuild the snapshot index from the snapshot files"
    )
    _profile_option(repair)
    repair.add_argument(
        "--dry-run", action="store_true", help="list the actions without doing them"
    )
    _display_options(repair, include_format=False)

    profiles = _command(subparsers, "profile", "manage scan profiles")
    profile_subparsers = profiles.add_subparsers(
        dest="profile_action", required=True, metavar="ACTION"
    )
    profile_list = _command(profile_subparsers, "list", "list configured profiles")
    _display_options(profile_list, include_format=False)
    profile_show = _command(profile_subparsers, "show", "show one profile's settings")
    profile_show.add_argument("name", help="profile name")
    _display_options(profile_show)
    profile_add = _command(
        profile_subparsers,
        "add",
        "create a profile or change the given settings of one",
    )
    profile_add.add_argument("name", help="profile name")
    # Options store into ProfileConfig fields and default to SUPPRESS, so only
    # given options are written; the rest keep their stored or [defaults] value.
    profile_add.add_argument(
        "--path",
        dest="root",
        type=_expanded_path,
        required=True,
        help="directory the profile scans",
    )
    profile_add.add_argument(
        "--exclude",
        dest="excludes",
        action="append",
        default=argparse.SUPPRESS,
        metavar="PATTERN",
        help="exclusion pattern; repeat to replace the stored list",
    )
    _retention_options(profile_add)
    _users_options(profile_add)
    profile_add.add_argument(
        "--user-max-depth",
        type=non_negative_int,
        default=argparse.SUPPRESS,
        metavar="N",
        help="record owner totals for directories down to depth N",
    )
    profile_add.add_argument(
        "--file-top",
        dest="large_file_limit",
        type=non_negative_int,
        default=argparse.SUPPRESS,
        metavar="N",
        help="remember the N largest files for 'qdu files'",
    )
    profile_add.add_argument(
        "--record-max-depth",
        type=non_negative_int,
        default=argparse.SUPPRESS,
        metavar="N",
        help="store directories down to depth N only",
    )
    _capacity_options(profile_add)
    _filesystem_options(profile_add)
    _display_options(profile_add, include_format=False)
    profile_remove = _command(profile_subparsers, "remove", "remove a profile")
    profile_remove.add_argument("name", help="profile name")
    profile_remove.add_argument(
        "--delete-data",
        action="store_true",
        help="also delete the profile's snapshots",
    )
    _display_options(profile_remove, include_format=False)

    configuration = _command(subparsers, "config", "show or create the configuration")
    config_subparsers = configuration.add_subparsers(
        dest="config_action", required=True, metavar="ACTION"
    )
    config_path = _command(config_subparsers, "path", "print the configuration path")
    _display_options(config_path, include_format=False)
    config_init = _command(
        config_subparsers, "init", "create a starter configuration file"
    )
    _display_options(config_init, include_format=False)
    config_show = _command(
        config_subparsers, "show", "show a profile's effective settings"
    )
    _profile_option(config_show)
    _display_options(config_show)

    return parser


def _command(
    subparsers: argparse._SubParsersAction[argparse.ArgumentParser],
    name: str,
    summary: str,
) -> argparse.ArgumentParser:
    return subparsers.add_parser(
        name, help=summary, description=f"{summary[0].upper()}{summary[1:]}."
    )


# Short metric names accepted on the command line; output keeps column names.
_METRIC_NAMES = {
    "allocated": "allocated_bytes",
    "apparent": "apparent_bytes",
    "inodes": "inode_count",
}
_METRIC_DESCRIPTIONS = {
    "allocated_bytes": "allocated bytes",
    "apparent_bytes": "apparent bytes",
    "inode_count": "inodes",
}


def _metric_option(
    parser: argparse.ArgumentParser, *metrics: str, purpose: str
) -> None:
    short_names = {column: name for name, column in _METRIC_NAMES.items()}
    shown = [short_names[metric] for metric in metrics]
    described = [_METRIC_DESCRIPTIONS[metric] for metric in metrics]

    def parse(value: str) -> str:
        metric = _METRIC_NAMES.get(value, value)
        if metric not in metrics:
            raise argparse.ArgumentTypeError(f"choose from {', '.join(shown)}")
        return metric

    parser.add_argument(
        "--metric",
        type=parse,
        default=metrics[0],
        metavar="{" + ",".join(shown) + "}",
        help=f"{purpose} {', '.join(described[:-1])} or {described[-1]} "
        f"(default: {shown[0]})",
    )


def clock_time(value: str) -> tuple[int, int]:
    """Parse a 24-hour HH:MM time of day for argparse."""
    hour, separator, minute = value.partition(":")
    if (
        separator
        and hour.isdigit()
        and minute.isdigit()
        and int(hour) < 24
        and int(minute) < 60
    ):
        return int(hour), int(minute)
    raise argparse.ArgumentTypeError("use a 24-hour time such as 03:15")


def compact_after(value: str) -> int | None:
    """Parse a compaction age or ``off`` for argparse."""
    try:
        return parse_compact_after(value)
    except QduError as exc:
        raise argparse.ArgumentTypeError(str(exc)) from exc


def _compact_after_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--compact-after",
        dest="compact_after_seconds",
        type=compact_after,
        default=argparse.SUPPRESS,
        metavar="DURATION",
        help="after a snapshot, gzip snapshots older than DURATION except the two "
        "newest; 'off' disables it (default: 7d)",
    )


def _profile_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--profile",
        default=os.environ.get("QDU_PROFILE", "default"),
        metavar="NAME",
        help="profile to use (default: $QDU_PROFILE, or 'default')",
    )


def _snapshot_selector(
    parser: argparse.ArgumentParser, *, default: str = "latest"
) -> None:
    parser.add_argument(
        "--snapshot",
        default=default,
        metavar="SELECTOR",
        help="latest (the newest complete snapshot), latest-any, previous, or a "
        f"snapshot ID or unique ID prefix (default: {default})",
    )


def _comparison_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--from",
        dest="from_snapshot",
        metavar="SELECTOR",
        help="older snapshot (default: the complete snapshot before --to)",
    )
    parser.add_argument(
        "--to",
        dest="to_snapshot",
        default="latest",
        metavar="SELECTOR",
        help="newer snapshot (default: latest)",
    )
    direction = parser.add_mutually_exclusive_group()
    direction.add_argument(
        "--growth-only", action="store_true", help="only show what grew"
    )
    direction.add_argument(
        "--shrink-only", action="store_true", help="only show what shrank"
    )
    parser.add_argument(
        "--order",
        choices=("growth", "shrink", "absolute", "current"),
        default="growth",
        help="rank by growth, shrinkage, size of the change, or current value "
        "(default: growth)",
    )


def _max_depth_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "-L",
        "--max-depth",
        type=positive_int,
        default=1,
        metavar="N",
        help="include directories up to N levels below the scope (default: 1)",
    )


def _top_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "-n",
        "--top",
        type=positive_int,
        default=20,
        metavar="N",
        help="show N rows (default: 20)",
    )


def _under_option(
    parser: argparse.ArgumentParser,
    help_text: str = "start from this directory, relative to the root "
    "(default: the root)",
) -> None:
    parser.add_argument("--under", default=".", metavar="PATH", help=help_text)


def _min_size_option(parser: argparse.ArgumentParser, help_text: str) -> None:
    parser.add_argument("--min-size", default="0", metavar="SIZE", help=help_text)


def _match_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--match",
        metavar="TEXT",
        help="keep paths containing TEXT (ignoring case) or matching a glob such "
        "as 'models/*'",
    )


def _ranking_options(
    parser: argparse.ArgumentParser, *, min_size_help: str | None
) -> None:
    _max_depth_option(parser)
    _top_option(parser)
    _under_option(parser)
    if min_size_help is None:
        parser.set_defaults(min_size="0")
    else:
        _min_size_option(parser, min_size_help)
    _match_option(parser)


def _warn_age_option(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--warn-age",
        metavar="DURATION",
        help="warn when the snapshot is older than DURATION, such as 8d",
    )


def _owner_report_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--users",
        action="store_true",
        help="also show the top owners and their directories (needs --with-users)",
    )
    parser.add_argument(
        "--user-top",
        type=positive_int,
        default=10,
        metavar="N",
        help="owners to show with --users (default: 10)",
    )
    parser.add_argument(
        "--user-dir-top",
        type=non_negative_int,
        default=5,
        metavar="N",
        help="directories per owner with --users (default: 5)",
    )


def _retention_options(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--keep-snapshots",
        type=non_negative_int,
        default=argparse.SUPPRESS,
        metavar="N",
        help="keep at most N snapshots, removing the oldest; 0 keeps all (default: 52)",
    )
    _compact_after_option(parser)


def _users_options(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--with-users",
        action="store_true",
        dest="collect_users",
        default=argparse.SUPPRESS,
        help="record usage per file owner, for 'qdu users'",
    )
    group.add_argument(
        "--without-users",
        action="store_false",
        dest="collect_users",
        default=argparse.SUPPRESS,
        help="do not record usage per file owner (default)",
    )


def _filesystem_options(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--cross-filesystems",
        action="store_true",
        dest="cross_filesystems",
        default=argparse.SUPPRESS,
        help="descend into directories mounted from other filesystems",
    )
    group.add_argument(
        "--one-file-system",
        action="store_false",
        dest="cross_filesystems",
        default=argparse.SUPPRESS,
        help="stay on the root's filesystem (default)",
    )


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
            "--format",
            choices=("table", "tsv", "json"),
            default="table",
            help="output format; TSV and JSON are stable for scripts (default: table)",
        )
    else:
        parser.set_defaults(format="table")
    parser.add_argument(
        "--style",
        choices=("auto", "rich", "plain"),
        default="auto",
        help="table drawing: rich box lines on a UTF-8 terminal, or plain "
        "(default: auto)",
    )
    parser.add_argument(
        "--color",
        choices=("auto", "always", "never"),
        default="auto",
        help="colored output (default: auto, on a terminal)",
    )


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
            return explain_command(args, config)
        if args.command == "browse":
            return browse_command(args, config)
        if args.command == "check":
            return check_command(args, config)
        if args.command == "schedule":
            return schedule_command(args, config)
        if args.command == "compact":
            return compact_command(args)
        if args.command == "verify":
            return verify_command(args)
        if args.command == "doctor":
            return doctor_command(args, config)
        if args.command == "repair":
            return repair_command(args)
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
