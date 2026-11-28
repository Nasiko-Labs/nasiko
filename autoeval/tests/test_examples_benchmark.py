"""Synthetic histories and explicit frozen membership are independent contracts."""

import json
import tempfile
import unittest
from pathlib import Path

from nasiko_autoeval.benchmark import collect_members
from nasiko_autoeval.examples import EXAMPLES, NAMES, prepare_example
from nasiko_autoeval.integrity import sha256, verify_checksums
from nasiko_autoeval.schema import EvalError, save_json
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
