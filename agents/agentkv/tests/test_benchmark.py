from decimal import Decimal
import unittest
from agentkv.accounting import RunCost
from agentkv.benchmark import Measurement, frontier


def point(name, latency, dollars, successes=10, failures=0, pending=0, cohort="same"):
    return Measurement(name, "A", cohort, latency, 100,
                       RunCost(Decimal(dollars), Decimal(0), Decimal(0), successes, failures, pending))


class FrontierTests(unittest.TestCase):
    def test_tradeoffs_dominance_and_ties(self):
        points = [point("fast", 1, "2"), point("cheap", 2, "1"), point("bad", 3, "3"),
                  point("tie", 1, "2")]
        self.assertEqual([p.run_id for p in frontier(points, minimum_success_rate=.9)],
                         ["fast", "tie", "cheap"])

    def test_no_success_pending_and_quality_failure_excluded(self):
        points = [point("zero", 1, "1", successes=0), point("pending", 1, "1", pending=1),
                  point("unreliable", 1, "1", failures=10)]
        self.assertEqual(frontier(points, minimum_success_rate=.9), [])

    def test_incomparable_runs_rejected(self):
        with self.assertRaises(ValueError):
            frontier([point("one", 1, "1"), point("two", 2, "2", cohort="different")],
                     minimum_success_rate=.9)

    def test_total_cost_includes_failures(self):
        p = point("run", 1, "2", successes=5, failures=5)
        self.assertEqual(p.dollars_per_thousand, Decimal(400))
