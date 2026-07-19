from __future__ import annotations


class QduError(Exception):
    """Base class for recoverable qdu failures."""

    exit_code = 1


class UsageError(QduError):
    exit_code = 2


class IncompleteSnapshotError(QduError):
    exit_code = 3


class SnapshotFormatError(QduError):
    exit_code = 4


class BusyError(QduError):
    exit_code = 5


class VerificationError(QduError):
    exit_code = 6


class ThresholdExceeded(QduError):
    exit_code = 10
