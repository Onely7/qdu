"""Expose qdu command handlers through a stable import surface."""

from qdu.commands.analysis import (
    browse_command,
    explain_command,
    files_command,
    inodes_command,
    users_command,
)
from qdu.commands.configuration import config_command, profile_command
from qdu.commands.operations import (
    check_command,
    compact_command,
    doctor_command,
    repair_command,
    unlock_command,
    verify_command,
)
from qdu.commands.snapshots import (
    diff_command,
    errors_command,
    list_command,
    show_command,
    snapshot_command,
)

__all__ = [
    "browse_command",
    "check_command",
    "compact_command",
    "config_command",
    "diff_command",
    "doctor_command",
    "errors_command",
    "explain_command",
    "files_command",
    "inodes_command",
    "list_command",
    "profile_command",
    "repair_command",
    "show_command",
    "snapshot_command",
    "unlock_command",
    "users_command",
    "verify_command",
]
