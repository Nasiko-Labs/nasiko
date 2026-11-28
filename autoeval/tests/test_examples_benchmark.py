"""Synthetic histories and explicit frozen membership are independent contracts."""

import json
import tempfile
import unittest
from pathlib import Path

from nasiko_autoeval.benchmark import collect_members, verify_benchmark
from nasiko_autoeval.examples import EXAMPLES, NAMES, prepare_example
from nasiko_autoeval.integrity import sha256, verify_checksums
from nasiko_autoeval.report import summarize
from nasiko_autoeval.results import grade
from nasiko_autoeval.schema import EvalError, TestRun, save_json
from nasiko_autoeval.tasks import build_task, git, model_prompt


class CalculatorHistoryTests(unittest.TestCase):
    def test_five_portable_histories_detect_and_split_with_no_reference_leak(self):
        with tempfile.TemporaryDirectory() as temp:
            for name in NAMES:
                with self.subTest(example=name):
                    entry = prepare_example(name, Path(temp) / "repos")
                    repeated = prepare_example(name, Path(temp) / "repos")
                    self.assertEqual(entry["fix_sha"], repeated["fix_sha"])
                    repo = entry["repository"]
                    self.assertEqual(
                        git(repo, "rev-list", "--count", "HEAD").decode().strip(), "2"
                    )
                    task, _, base, _, _ = build_task(
                        repo, entry["fix_sha"], Path(temp) / "tasks" / name
                    )
                    self.assertEqual(task["input_kind"], "local_fixture")
                    self.assertEqual(len(task["production_changes"]), 1)
                    self.assertEqual(len(task["test_changes"]), 1)
                    self.assertNotIn("reference.patch", base)
                    self.assertNotIn("base-tests.txt", base)
                    prompt = model_prompt(task, base, 80000)
                    self.assertNotIn("a + b", prompt)
                    self.assertNotIn("test_positive", prompt)
                    self.assertTrue((EXAMPLES / name / "history.json").is_file())


class FrozenMembershipTests(unittest.TestCase):
    def setup_member(self, directory, name="real"):
        run = {
            "task": {
                "input_kind": "real_repository",
                "fix_sha": "f",
                "parent_sha": "p",
                "repository_url": "https://example.invalid/repo",
            },
            "validation": {"status": "verified"},
            "attempts": [{"model": "m", "outcome": "resolved", "cost_usd": 0.01}],
            "preflight": [],
        }
        save_json(directory / name / "results.json", run)
        entry = {
            "run": name,
            "fix_sha": "f",
            "parent_sha": "p",
            "repository_url": "https://example.invalid/repo",
        }
        planned = {
            "name": "frozen",
            "models": ["provider:m"],
            "tasks": [entry],
            "grading_version": "host-stream-v2",
        }
        return run, planned

    def test_development_rerun_and_synthetic_neighbor_do_not_enter_headline(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            run, planned = self.setup_member(root)
            save_json(
                root / "unlisted-development" / "results.json",
                {**run, "attempts": run["attempts"] * 50},
            )
            save_json(
                root / "unlisted-synthetic" / "results.json",
                {"task": {"input_kind": "local_fixture"}},
            )
            result = collect_members(root, planned)
            self.assertEqual(len(result["attempts"]), 1)
            self.assertEqual(result["summary"][0]["cost_usd"], 0.01)

    def test_synthetic_or_mismatched_member_and_duplicate_models_rejected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            run, planned = self.setup_member(root)
            for field, value in (("input_kind", "local_fixture"), ("fix_sha", "other")):
                changed = {**run, "task": {**run["task"], field: value}}
                save_json(root / "real" / "results.json", changed)
                with self.assertRaises(EvalError):
                    collect_members(root, planned)
            save_json(
                root / "real" / "results.json", {**run, "attempts": run["attempts"] * 2}
            )
            with self.assertRaises(EvalError):
                collect_members(root, planned)

    def test_changed_or_missing_sealed_artifact_is_detected(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / "result.json"
            path.write_text(json.dumps({"passed": False}))
            hashes = {"result.json": sha256(path)}
            verify_checksums(root, hashes)
            path.write_text(json.dumps({"passed": True}))
            with self.assertRaises(EvalError):
                verify_checksums(root, hashes)
            path.unlink()
            with self.assertRaises(EvalError):
                verify_checksums(root, hashes)


class FrozenScoringTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.frozen = verify_benchmark(Path(__file__).resolve().parents[1] / "final")
        cls.gates = {
            item["task"]["fix_sha"]: item["validation"]
            for item in cls.frozen["verified_tasks"]
        }

    def test_final_scores_and_costs_follow_captured_test_evidence(self):
        attempts = []
        for attempt in self.frozen["attempts"]:
            verdict = grade(TestRun(**attempt["test_run"]), self.gates[attempt["task"]])
            self.assertEqual(verdict["outcome"], attempt["outcome"])
            attempts.append({**attempt, **verdict})
        summary = summarize(attempts)
        self.assertEqual(summary, self.frozen["summary"])
        self.assertEqual(
            [
                (r["model"], r["resolved"], r["attempted"], f"{r['cost_usd']:.6f}")
                for r in summary
            ],
            [
                ("gemini-3.1-flash-lite", 2, 3, "0.016999"),
                ("gemini-3.5-flash-lite", 2, 3, "0.025345"),
            ],
        )

    def test_model_labels_cannot_override_failing_test_evidence(self):
        for row in self.frozen["summary"]:
            model = row["model"]
            attempts = [a for a in self.frozen["attempts"] if a["model"] == model]
            passing = next(a for a in attempts if a["outcome"] == "resolved")
            captured = passing["test_run"]
            tests = dict(captured["tests"])
            tests[self.gates[passing["task"]]["expected_tests"][0]] = "failed"
            failed_run = TestRun(
                **{**captured, "status": "test_failure", "tests": tests, "exit_code": 1}
            )
            verdict = grade(failed_run, self.gates[passing["task"]])
            self.assertEqual(verdict["outcome"], "unresolved")
            changed = [{**a, **verdict} if a is passing else dict(a) for a in attempts]
            for label in (model, "anonymous-model"):
                with self.subTest(model=model, label=label):
                    original = summarize([{**a, "model": label} for a in attempts])[0]
                    updated = summarize([{**a, "model": label} for a in changed])[0]
                    self.assertEqual(original["resolved"], 2)
                    self.assertEqual(updated["resolved"], 1)
                    self.assertEqual(updated["attempted"], 3)
                    self.assertEqual(updated["cost_usd"], row["cost_usd"])
