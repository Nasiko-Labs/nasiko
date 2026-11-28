"""Hermetic contract tests; local fixtures are not real-world benchmark results."""

import json
import tempfile
import unittest
from pathlib import Path

from nasiko_autoeval.analyzer import inspect_repository, is_test, select_plan
from nasiko_autoeval.report import summarize
from nasiko_autoeval.results import grade, normalize, parse_results, verify_gate
from nasiko_autoeval.schema import EvalError, TestRun
from nasiko_autoeval.tasks import apply_candidate, model_prompt


class RepositoryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def write(self, path, text):
        path = self.root / path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def python(self):
        self.write(
            "pyproject.toml",
            '[project]\nname="example"\nrequires-python=">=3.11"\n[tool.pytest.ini_options]\ntestpaths=["testing"]\n',
        )
        return select_plan(inspect_repository(self.root))

    def test_extensions_alone_do_not_identify_framework(self):
        self.write("foo.py", "pass\n")
        self.write("foo.ts", "export const x = 1;\n")
        self.assertEqual(inspect_repository(self.root), [])

    def test_pytest_manifest_and_custom_testpaths(self):
        plan = self.python()
        self.assertEqual(plan.framework, "pytest")
        self.assertTrue(is_test("testing/check_parser.py", plan))
        self.assertIn("testing", plan.test_command)
        self.assertEqual(plan.runtime_requirements["requires-python"], ">=3.11")

    def test_pytest_ini_without_project_metadata(self):
        self.write("pytest.ini", "[pytest]\ntestpaths=checks\n")
        plan = select_plan(inspect_repository(self.root))
        self.assertTrue(is_test("checks/scenario.py", plan))

    def test_four_toolchains_share_schema_and_monorepo_selection(self):
        self.python()
        self.write(
            "js/package.json", json.dumps({"devDependencies": {"jest": "29.7.0"}})
        )
        self.write(
            "ts/package.json",
            json.dumps({"devDependencies": {"vitest": "2.1.9", "typescript": "5.7.3"}}),
        )
        self.write("go/go.mod", "module example.com/test\ngo 1.24\n")
        self.write(
            "rust/Cargo.toml",
            '[package]\nname="example"\nversion="0.1.0"\nedition="2021"\n',
        )
        plans = inspect_repository(self.root)
        self.assertEqual(len(plans), 5)
        self.assertEqual(
            {p.framework for p in plans},
            {"pytest", "jest", "vitest", "go-test", "cargo"},
        )
        self.assertTrue(all(set(p.to_dict()) == set(plans[0].to_dict()) for p in plans))
        with self.assertRaises(EvalError):
            select_plan(plans)
        self.assertEqual(select_plan(plans, "ts:vitest").language, "typescript")

    def test_ambiguous_node_framework_requires_framework_selection(self):
        self.write(
            "package.json",
            json.dumps({"devDependencies": {"jest": "29", "vitest": "2"}}),
        )
        plans = inspect_repository(self.root)
        with self.assertRaises(EvalError):
            select_plan(plans, ".")
        self.assertEqual(select_plan(plans, ".:jest").framework, "jest")

    def test_npm_lock_and_other_package_manager_explicit_override(self):
        self.write("package.json", '{"devDependencies":{"vitest":"2.1.9"}}')
        self.write("package-lock.json", "{}")
        plan = select_plan(inspect_repository(self.root))
        self.assertEqual(plan.setup_commands, ["npm ci --ignore-scripts"])
        self.write("pnpm-lock.yaml", "lockfileVersion: '9.0'")
        plan = select_plan(inspect_repository(self.root))
        self.assertEqual(plan.setup_commands, [])
        self.assertEqual(
            select_plan(
                [plan], overrides={"setup_commands": ["corepack pnpm install"]}
            ).setup_commands,
            ["corepack pnpm install"],
        )

    def test_virtual_cargo_workspace_exposes_concrete_members(self):
        self.write("Cargo.toml", '[workspace]\nmembers=["member"]\n')
        self.write("rust-toolchain.toml", '[toolchain]\nchannel="1.88"\n')
        self.write("member/Cargo.toml", '[package]\nname="m"\nversion="0.1.0"\n')
        plan = select_plan(inspect_repository(self.root))
        self.assertEqual(plan.working_directory, "member")
        self.assertEqual(plan.runtime_image, "rust:1.88-trixie")

    def test_execution_override_validation(self):
        plan = self.python()
        for override in (
            {"framework": "anything"},
            {"unknown": 1},
            {"timeout_seconds": 0},
        ):
            with self.subTest(override=override), self.assertRaises(EvalError):
                select_plan([plan], overrides=override)


class ParserTests(unittest.TestCase):
    def test_junit_pass_fail_skip_parameter_ids(self):
        xml = '<testsuites><testsuite errors="0"><testcase classname="parser" name="test_x[a]"/><testcase classname="parser" name="test_x[b]"><failure/></testcase><testcase classname="parser" name="test_skip"><skipped/></testcase></testsuite></testsuites>'
        self.assertEqual(
            parse_results("junit", xml, ""),
            {
                "parser::test_x[a]": "passed",
                "parser::test_x[b]": "failed",
                "parser::test_skip": "skipped",
            },
        )

    def test_junit_setup_error_is_infrastructure(self):
        with self.assertRaisesRegex(EvalError, "setup"):
            parse_results(
                "junit",
                '<testsuite><testcase name="x"><error/></testcase></testsuite>',
                "",
            )

    def test_duplicate_junit_identity_is_rejected(self):
        with self.assertRaisesRegex(EvalError, "Duplicate"):
            parse_results(
                "junit",
                '<testsuite><testcase name="x"/><testcase name="x"/></testsuite>',
                "",
            )

    def test_jest_and_vitest_normalize_same_json(self):
        data = {
            "numTotalTests": 2,
            "testResults": [
                {
                    "name": "/work/test/a.test.ts",
                    "assertionResults": [
                        {"fullName": "path handles root", "status": "passed"},
                        {"fullName": "path handles drive", "status": "pending"},
                    ],
                }
            ],
        }
        self.assertEqual(
            parse_results("jest", json.dumps(data), ""),
            {
                "test/a.test.ts::path handles root": "passed",
                "test/a.test.ts::path handles drive": "skipped",
            },
        )
        data["numTotalTests"] = 3
        with self.assertRaises(EvalError):
            parse_results("jest", json.dumps(data), "")

    def test_jest_suite_load_error_is_not_a_test_failure(self):
        with self.assertRaises(EvalError):
            parse_results(
                "jest",
                '{"testResults":[{"name":"a","status":"failed","assertionResults":[]}]}',
                "",
            )

    def test_repeated_parameterized_js_titles_keep_every_case(self):
        data = {
            "numTotalTests": 2,
            "testResults": [
                {
                    "name": "/work/a.test.ts",
                    "assertionResults": [
                        {"fullName": "duplicate", "status": "passed"},
                        {"fullName": "duplicate", "status": "failed"},
                    ],
                }
            ],
        }
        results = parse_results("jest", json.dumps(data), "")
        self.assertEqual(
            results,
            {
                "a.test.ts::duplicate#occurrence=1": "passed",
                "a.test.ts::duplicate#occurrence=2": "failed",
            },
        )

    def test_go_subtests_and_package_completion(self):
        events = [
            {"Package": "p", "Action": "start"},
            {"Package": "p", "Test": "TestFormat/a", "Action": "pass"},
            {"Package": "p", "Test": "TestFormat", "Action": "pass"},
            {"Package": "p", "Action": "pass"},
        ]
        out = "\n".join(map(json.dumps, events))
        self.assertEqual(len(parse_results("go-json", None, out)), 2)
        with self.assertRaisesRegex(EvalError, "incomplete"):
            parse_results("go-json", None, "\n".join(map(json.dumps, events[:-1])))

    def test_go_compile_failure_is_not_reproduction(self):
        with self.assertRaisesRegex(EvalError, "build/setup"):
            parse_results("go-json", None, '{"Package":"p","Action":"fail"}')

    def test_cargo_namespaced_by_binary_matching_existing_contract(self):
        out = "Running unittests src/lib.rs (target/debug/deps/sample-abc123)\ntest tests::it ... ok\ntest result: ok. 1 passed\nRunning tests/it.rs (target/debug/deps/it-abc123)\ntest tests::it ... FAILED\ntest result: FAILED. 1 failed\n"
        self.assertEqual(
            parse_results("cargo", None, out),
            {"sample::tests::it": "passed", "it::tests::it": "failed"},
        )

    def test_cargo_compile_failure_not_reproduction(self):
        with self.assertRaisesRegex(EvalError, "build"):
            parse_results("cargo", None, "error: could not compile `sample`")


class GradingTests(unittest.TestCase):
    def setUp(self):
        self.before = TestRun(
            "test_failure", {"regression": "failed", "existing": "passed"}, 1, 0.1
        )
        self.after = TestRun(
            "passed", {"regression": "passed", "existing": "passed"}, 0, 0.1
        )
        self.validation = verify_gate(
            [self.before, self.before], [self.after, self.after]
        )

    def test_shared_gate_records_exact_identities(self):
        self.assertEqual(self.validation["fail_to_pass"], ["regression"])
        self.assertEqual(self.validation["pass_to_pass"], ["existing"])
        self.assertEqual(grade(self.after, self.validation)["outcome"], "resolved")

    def test_one_repeat_is_not_verification(self):
        with self.assertRaises(EvalError):
            verify_gate([self.before], [self.after])

    def test_flakiness_rejected(self):
        with self.assertRaisesRegex(EvalError, "Flaky"):
            verify_gate([self.before, self.after], [self.after, self.after])

    def test_setup_failure_never_proves_bug(self):
        run = TestRun("infrastructure_error", {}, 2, 0.1, "missing dependency")
        with self.assertRaisesRegex(EvalError, "infrastructure"):
            verify_gate([run, run], [self.after, self.after])

    def test_candidate_compile_failure_is_unresolved_and_cannot_validate_task(self):
        run = TestRun("suite_error", {}, 1, 0.1, "Syntax error")
        self.assertEqual(grade(run, self.validation)["outcome"], "unresolved")
        with self.assertRaises(EvalError):
            verify_gate([run, run], [self.after, self.after])

    def test_missing_zero_skipped_timeout_and_failed_suite_never_resolve(self):
        runs = [
            TestRun("passed", {}, 0, 1),
            TestRun("passed", {"regression": "passed"}, 0, 1),
            TestRun("passed", {"regression": "skipped", "existing": "passed"}, 0, 1),
            TestRun("timeout", {}, None, 1),
            self.before,
        ]
        for run in runs:
            with self.subTest(run=run):
                self.assertNotEqual(grade(run, self.validation)["outcome"], "resolved")

    def test_non_regression_suite_failure_prevents_resolution(self):
        run = TestRun(
            "test_failure", {"regression": "passed", "existing": "failed"}, 1, 1
        )
        self.assertEqual(grade(run, self.validation)["outcome"], "unresolved")

    def test_zero_collection_and_missing_reports_are_infrastructure(self):
        from nasiko_autoeval.schema import ExecutionPlan

        plan = ExecutionPlan(
            "python", "pytest", "python:3.12-slim", ".", [], "pytest", "junit", [], []
        )
        for report in (None, "", "<testsuite/>"):
            self.assertEqual(
                normalize(plan, 0, 1, report, "", "a").status, "infrastructure_error"
            )

    def test_exit_report_disagreement_is_infrastructure(self):
        from nasiko_autoeval.schema import ExecutionPlan

        plan = ExecutionPlan(
            "python", "pytest", "python:3.12-slim", ".", [], "pytest", "junit", [], []
        )
        xml = '<testsuite><testcase name="x"><failure/></testcase></testsuite>'
        self.assertEqual(
            normalize(plan, 0, 1, xml, "", "a").status, "infrastructure_error"
        )

    def test_cost_coverage_and_zero_resolutions(self):
        rows = [
            {"model": "m", "outcome": "unresolved", "cost_usd": 0.01},
            {"model": "m", "outcome": "infrastructure_error", "cost_usd": None},
        ]
        summary = summarize(rows)[0]
        self.assertEqual(summary["cost_coverage"], "1/2")
        self.assertIsNone(summary["cost_per_attempt_usd"])
        self.assertIsNone(summary["cost_per_resolution_usd"])
        self.assertEqual(summary["infrastructure_failures"], 1)


class CandidateBoundaryTests(unittest.TestCase):
    def setUp(self):
        from nasiko_autoeval.schema import ExecutionPlan

        self.plan = ExecutionPlan(
            "python",
            "pytest",
            "python:3.12-slim",
            ".",
            [],
            "pytest",
            "junit",
            ["tests/**"],
            [],
        )
        self.base = {
            "src/a.py": b"value = 1\n",
            "tests/test_a.py": b"assert False\n",
            "pyproject.toml": b"protected\n",
        }
        self.task = {
            "allowed_paths": ["src/a.py"],
            "issue": "Handle empty input",
            "plan": self.plan.to_dict(),
        }

    def test_test_configuration_and_unlisted_changes_rejected(self):
        for path in (
            "tests/test_a.py",
            "pyproject.toml",
            "src/new.py",
            "../x",
            "/tmp/x",
            ".env",
            ".git/config",
        ):
            with self.subTest(path=path), self.assertRaises(EvalError):
                apply_candidate(
                    self.base,
                    [{"path": path, "content": "malicious"}],
                    self.task,
                    self.plan,
                )

    def test_duplicates_rejected_and_trusted_assets_restored(self):
        edit = {"path": "src/a.py", "content": "value = 2\n"}
        with self.assertRaises(EvalError):
            apply_candidate(self.base, [edit, edit], self.task, self.plan)
        result = apply_candidate(self.base, [edit], self.task, self.plan)
        self.assertEqual(result["tests/test_a.py"], self.base["tests/test_a.py"])
        self.assertEqual(result["pyproject.toml"], self.base["pyproject.toml"])

    def test_prompt_contains_parent_production_source_only(self):
        self.task["gold_patch"] = "TOP_SECRET_REFERENCE"
        prompt = model_prompt(self.task, self.base, 1000)
        for hidden in (
            "TOP_SECRET_REFERENCE",
            "assert False",
            "protected",
            "tests/test_a.py",
        ):
            self.assertNotIn(hidden, prompt)
        self.assertIn("value = 1", prompt)

    def test_oversized_context_rejected_instead_of_model_specific_truncation(self):
        with self.assertRaises(EvalError):
            model_prompt(self.task, self.base, 1)


class UsageEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.row = {
            "model": "example",
            "input_tokens": 10,
            "output_tokens": 2,
            "cost_usd": 0.001,
            "pricing": {"source": "Exact", "estimated": False},
        }

    def test_only_non_estimated_attributable_recorded_cost_is_accepted(self):
        from nasiko_autoeval.nasiko import verified_cost

        self.assertEqual(verified_cost(self.row, "example"), 0.001)
        modern = {
            "models": ["example"],
            "input_tokens": 10,
            "output_tokens": 2,
            "cost_usd": 0.001,
            "worst_price_source": "Exact",
            "estimated_calls": 0,
        }
        self.assertEqual(verified_cost(modern, "example"), 0.001)

    def test_estimated_missing_zero_usage_and_wrong_model_never_become_cost(self):
        from nasiko_autoeval.nasiko import verified_cost

        cases = [
            {**self.row, "cost_estimated": True},
            {**self.row, "input_tokens": 0, "output_tokens": 0},
            {**self.row, "model": "fallback"},
            {**self.row, "cost_usd": None},
            {**self.row, "cost_usd": float("nan")},
            {**self.row, "pricing": {}},
            {**self.row, "pricing": {"source": "Static", "estimated": True}},
        ]
        for row in cases:
            with self.subTest(row=row), self.assertRaises(EvalError):
                verified_cost(row, "example")

    def test_snapshot_delta_checks_response_tokens(self):
        from nasiko_autoeval.nasiko import NasikoClient

        client = NasikoClient.__new__(NasikoClient)
        client.modern = False
        row = {
            **self.row,
            "trace_id": "direct:new",
            "cache_read_tokens": 0,
            "cache_creation_tokens": 0,
        }
        client._ledger_rows = lambda: [{**row, "trace_id": "direct:old"}, row]
        response = {"usage": {"prompt_tokens": 10, "completion_tokens": 2}}
        self.assertEqual(
            client._usage("run:task", {"direct:old"}, response)["trace_id"],
            "direct:new",
        )
        with self.assertRaisesRegex(EvalError, "disagree"):
            client._usage(
                "run:task",
                {"direct:old"},
                {"usage": {"prompt_tokens": 99, "completion_tokens": 2}},
            )

    def test_concurrent_identity_traffic_makes_cost_ambiguous(self):
        from nasiko_autoeval.nasiko import NasikoClient

        client = NasikoClient.__new__(NasikoClient)
        client.modern = False
        client._ledger_rows = lambda: [
            {**self.row, "trace_id": "one"},
            {**self.row, "trace_id": "two"},
        ]
        with self.assertRaisesRegex(EvalError, "Ambiguous"):
            client._usage("run:task", set(), {})


if __name__ == "__main__":
    unittest.main()
