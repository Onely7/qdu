"""Assess recorded usage against an operational capacity limit."""

from __future__ import annotations

from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class CapacityPolicy:
    """Define a positive byte limit and its optional owner target."""

    limit_bytes: int
    user: str | None = None

    def __post_init__(self) -> None:
        if self.limit_bytes <= 0:
            raise ValueError("capacity limit must be greater than zero")
        if self.user is not None and not self.user.strip():
            raise ValueError("capacity user must not be empty")


@dataclass(frozen=True, slots=True)
class CapacityAssessment:
    """Describe observed usage relative to an operational limit."""

    target: str
    limit_bytes: int
    used_bytes: int

    @property
    def usage_percent(self) -> float:
        """Usage as a percentage of the configured limit."""
        return self.used_bytes * 100 / self.limit_bytes

    @property
    def exceeded(self) -> bool:
        """Whether observed usage is greater than the limit."""
        return self.used_bytes > self.limit_bytes

    @property
    def remaining_bytes(self) -> int:
        """Unused bytes, clamped to zero after the limit is exceeded."""
        return max(0, self.limit_bytes - self.used_bytes)

    @property
    def excess_bytes(self) -> int:
        """Bytes above the limit, or zero while usage remains within it."""
        return max(0, self.used_bytes - self.limit_bytes)

    def to_dict(self) -> dict[str, object]:
        """Return a JSON-serializable representation of the assessment."""
        return {
            "target": self.target,
            "limit_bytes": self.limit_bytes,
            "used_bytes": self.used_bytes,
            "usage_percent": self.usage_percent,
            "remaining_bytes": self.remaining_bytes,
            "excess_bytes": self.excess_bytes,
            "exceeded": self.exceeded,
        }


def assess_capacity(
    *, target: str, limit_bytes: int, used_bytes: int
) -> CapacityAssessment:
    """Create an assessment while treating negative observed usage as zero.

    Raises:
        ValueError: If the capacity limit is not positive.
    """
    return CapacityAssessment(
        target=target,
        limit_bytes=limit_bytes,
        used_bytes=max(0, used_bytes),
    )
