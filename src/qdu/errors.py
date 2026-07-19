"""Define recoverable qdu failures and their stable process exit codes."""

from __future__ import annotations


class QduError(Exception):
    """Base class for recoverable qdu failures."""

    exit_code = 1


class UsageError(QduError):
    """Report invalid command input or configuration with exit code 2."""

    exit_code = 2


class IncompleteSnapshotError(QduError):
    """Report a saved but incomplete snapshot with exit code 3."""

    exit_code = 3


class SnapshotFormatError(QduError):
    """Report malformed or incompatible stored data with exit code 4."""

    exit_code = 4


class BusyError(QduError):
    """Report a conflicting profile operation with exit code 5."""

    exit_code = 5


class VerificationError(QduError):
    """Report failed integrity verification with exit code 6."""

    exit_code = 6


class ThresholdExceeded(QduError):
    """Signal an operational threshold alert with exit code 10."""

    exit_code = 10
