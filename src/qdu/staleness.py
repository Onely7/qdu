"""Classify filesystem content by age at snapshot time."""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

_SECONDS_PER_DAY = 24 * 60 * 60


class StaleLevel(str, Enum):
    """Ordered labels used to present content inactivity."""

    RECENT = "recent"
    NOTICE = "notice"
    WARNING = "warning"
    ELEVATED = "elevated"
    CRITICAL = "critical"


@dataclass(frozen=True, slots=True)
class StaleThresholds:
    """Four strictly increasing day boundaries for inactivity labels."""

    notice_days: int = 30
    warning_days: int = 90
    elevated_days: int = 180
    critical_days: int = 365

    def __post_init__(self) -> None:
        values = (
            self.notice_days,
            self.warning_days,
            self.elevated_days,
            self.critical_days,
        )
        if any(value < 0 for value in values):
            raise ValueError("stale thresholds must be non-negative")
        if values != tuple(sorted(values)) or len(set(values)) != len(values):
            raise ValueError(
                "stale thresholds must be four strictly increasing day values"
            )

    @classmethod
    def parse(cls, value: str) -> StaleThresholds:
        """Parse four comma-separated day boundaries.

        Raises:
            ValueError: If four increasing non-negative integers are not given.
        """
        parts = [part.strip() for part in value.split(",")]
        if len(parts) != 4 or any(not part.isdigit() for part in parts):
            raise ValueError(
                "stale thresholds must contain four comma-separated integers, "
                "for example 30,90,180,365"
            )
        return cls(*(int(part) for part in parts))

    def as_text(self) -> str:
        """Serialize thresholds in the command-line representation."""
        return ",".join(
            str(value)
            for value in (
                self.notice_days,
                self.warning_days,
                self.elevated_days,
                self.critical_days,
            )
        )

    def classify(self, age_days: int) -> StaleLevel:
        """Return the level reached by an age in whole days."""
        if age_days >= self.critical_days:
            return StaleLevel.CRITICAL
        if age_days >= self.elevated_days:
            return StaleLevel.ELEVATED
        if age_days >= self.warning_days:
            return StaleLevel.WARNING
        if age_days >= self.notice_days:
            return StaleLevel.NOTICE
        return StaleLevel.RECENT


@dataclass(frozen=True, slots=True)
class StaleAssessment:
    """Pair a whole-day age with its presentation level."""

    age_days: int
    level: StaleLevel

    @property
    def warning_label(self) -> str:
        """Compact age label, omitted for recent content."""
        return "" if self.level is StaleLevel.RECENT else f"{self.age_days}d"


DEFAULT_STALE_THRESHOLDS = StaleThresholds()


def assess_staleness(
    latest_modified_epoch: float,
    *,
    reference_epoch: int,
    thresholds: StaleThresholds = DEFAULT_STALE_THRESHOLDS,
) -> StaleAssessment:
    """Assess modification time relative to a snapshot reference epoch."""
    age_seconds = max(0.0, float(reference_epoch) - float(latest_modified_epoch))
    age_days = int(age_seconds // _SECONDS_PER_DAY)
    return StaleAssessment(age_days=age_days, level=thresholds.classify(age_days))


def stale_cutoff_epoch(
    reference_epoch: int, minimum_age_days: int | None
) -> float | None:
    """Return the newest allowed modification epoch for an age filter.

    Returns:
        The cutoff epoch, or ``None`` when filtering is disabled.

    Raises:
        ValueError: If the minimum age is negative.
    """
    if minimum_age_days is None:
        return None
    if minimum_age_days < 0:
        raise ValueError("minimum stale age must be non-negative")
    return float(reference_epoch - (minimum_age_days * _SECONDS_PER_DAY))
