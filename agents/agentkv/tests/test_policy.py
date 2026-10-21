import unittest
from agentkv.policy import check_support, engine_idle, retention_score, should_warm


class PolicyTests(unittest.TestCase):
    def test_retention_score_ranks_repair_risk_times_avoided_work(self):
        self.assertGreater(retention_score(.8, 100, 10), retention_score(.1, 100, 10))
        with self.assertRaises(ValueError):
            retention_score(.5, 0, 10)

    def test_warming_only_when_idle_and_reviewer_prefix_is_likely(self):
        self.assertTrue(should_warm(method="E", warming_enabled=True, idle=True,
                                    repair_probability=.2, reuse_prefix_probability=.9))
        self.assertFalse(should_warm(method="A", warming_enabled=True, idle=True,
                                     repair_probability=.2, reuse_prefix_probability=.9))
        self.assertFalse(should_warm(method="E", warming_enabled=True, idle=False,
                                     repair_probability=.2, reuse_prefix_probability=.9))
        self.assertFalse(should_warm(method="E", warming_enabled=True, idle=None,
                                     repair_probability=.2, reuse_prefix_probability=.9))
        self.assertFalse(should_warm(method="E", warming_enabled=True, idle=True,
                                     repair_probability=.9, reuse_prefix_probability=.9))
        self.assertFalse(should_warm(method="C", warming_enabled=False, idle=True,
                                     repair_probability=.2, reuse_prefix_probability=1))

    def test_unknown_engine_load_does_not_speculate(self):
        self.assertIsNone(engine_idle(""))
        self.assertTrue(engine_idle('vllm:num_requests_running 0\nvllm:num_requests_waiting 0'))
        self.assertFalse(engine_idle('vllm:num_requests_running{engine="0"} 2\nvllm:num_requests_waiting 0'))

    def test_support_checker_requires_label_and_policy_phrases(self):
        task = {"label": "shipping", "required_phrases": ["5 business days", "tracking"]}
        self.assertTrue(check_support(task, "shipping.", "Tracking is emailed. Delivery is 5 business days.")["passed"])
        self.assertFalse(check_support(task, "billing", "5 business days tracking")["passed"])
        self.assertFalse(check_support(task, "shipping", "we shipped it")["passed"])
