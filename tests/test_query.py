"""Test query boundary validation."""

from __future__ import annotations

import unittest
from typing import Any, cast

from qdu.errors import UsageError
from qdu.query import SnapshotQueryService


class _FailIfQueried:
    def execute(self, *_args: object, **_kwargs: object) -> None:
        raise AssertionError("invalid metrics must be rejected before SQL execution")


class QueryBoundaryTest(unittest.TestCase):
    def test_rejects_invalid_directory_metric_before_querying(self) -> None:
        service = SnapshotQueryService("default")

        with self.assertRaises(UsageError):
            service.directory_ranking(
                cast(Any, _FailIfQueried()),
                under=".",
                max_depth=1,
                top=10,
                min_size=0,
                match=None,
                metric=cast(Any, "allocated_bytes; DROP TABLE directories"),
            )

    def test_rejects_invalid_owner_metric_before_querying(self) -> None:
        service = SnapshotQueryService("default")

        with self.assertRaises(UsageError):
            service.owners(
                cast(Any, _FailIfQueried()),
                top=10,
                directories_per_user=5,
                metric=cast(Any, "uid DESC"),
            )
