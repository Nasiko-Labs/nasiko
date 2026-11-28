"""Focused checks for the new UI wrapper and its executable quality gate."""

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("live_ui_server", Path(__file__).with_name("server.py"))
SERVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SERVER)


def result(base="resolved", candidate="resolved"):
    return {"validation": {"status": "verified"}, "attempts": [
        {"model": "baseline", "task": "a", "outcome": base},
        {"model": "candidate", "task": "a", "outcome": candidate},
    ]}


class GateTests(unittest.TestCase):
    def check_gate(self, data):
        return SERVER.evaluate_gate(data, "provider:baseline", "provider:candidate")

    def test_quality_regression_blocks(self):
        self.assertEqual(self.check_gate(result(candidate="unresolved"))["decision"], "BLOCK")

    def test_same_success_passes(self):
        self.assertEqual(self.check_gate(result())["decision"], "PASS")

    def test_no_baseline_success_is_inconclusive(self):
        self.assertEqual(self.check_gate(result(base="unresolved", candidate="unresolved"))["decision"], "INCONCLUSIVE")

    def test_infrastructure_never_passes(self):
        self.assertEqual(self.check_gate(result(candidate="model_error"))["decision"], "INCONCLUSIVE")

    def test_different_task_sets_never_pass(self):
        data = result()
        data["attempts"][1]["task"] = "different"
        self.assertEqual(self.check_gate(data)["decision"], "INCONCLUSIVE")

    def test_duplicates_never_pass(self):
        data = result()
        data["attempts"].append(data["attempts"][0])
        self.assertEqual(self.check_gate(data)["decision"], "INCONCLUSIVE")

    def test_unverified_never_passes(self):
        data = result()
        data["validation"]["status"] = "rejected"
        self.assertEqual(self.check_gate(data)["decision"], "INCONCLUSIVE")

    def test_equal_aggregate_score_still_blocks_lost_task(self):
        data = result(candidate="unresolved")
        data["attempts"] += [
            {"model": "baseline", "task": "b", "outcome": "unresolved"},
            {"model": "candidate", "task": "b", "outcome": "resolved"},
        ]
        self.assertEqual(self.check_gate(data)["decision"], "BLOCK")


if __name__ == "__main__":
    unittest.main()
