import unittest
from decimal import Decimal as D
from agentkv.accounting import RunCost


class RunCostTests(unittest.TestCase):
    def test_failed_and_pending_work_are_not_successes(self):
        run = RunCost(D("2"), D("0.2"), D("0.1"), 2, 3, 5)
        self.assertEqual(run.cost_per_success, D("1.15"))

    def test_no_success_is_not_reported_as_free(self):
        self.assertIsNone(RunCost(D("2"), D("0"), D("0"), 0, 3, 0).cost_per_success)

    def test_zero_spend_is_valid_when_measured(self):
        self.assertEqual(RunCost(D("0"), D("0"), D("0"), 1, 0, 0).cost_per_success, D("0"))

    def test_invalid_costs_cannot_enter_report(self):
        for cost in (D("NaN"), D("Infinity"), D("-0.1"), 0.1):
            with self.subTest(cost=cost), self.assertRaises(ValueError):
                RunCost(cost, D("0"), D("0"), 1, 0, 0)

    def test_counts_are_nonnegative_integers(self):
        for count in (-1, 0.5, True):
            with self.subTest(count=count), self.assertRaises(ValueError):
                RunCost(D("1"), D("0"), D("0"), count, 0, 0)


if __name__ == "__main__":
    unittest.main()
