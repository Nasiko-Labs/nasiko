"""Normalize structured reports and enforce the same fail-closed grade everywhere."""

import json
import re
import xml.etree.ElementTree as ET
from collections import Counter

from .schema import EvalError, TestRun


class SuiteLoadError(EvalError):
    """Code could not compile/import/initialize the selected suite."""


def parse_results(kind, report, output):
    try:
        if kind == "junit":
            return _junit(report)
        if kind == "jest":
            return _jest(report)
        if kind == "go-json":
            return _go(output)
        if kind == "cargo":
            return _cargo(output)
    except (ValueError, KeyError, TypeError, ET.ParseError) as error:
        raise EvalError(f"Invalid {kind} result report: {error}") from None
    raise EvalError(f"Unsupported result format: {kind}")


def _insert(tests, identity, status):
    if identity in tests:
        raise EvalError(f"Duplicate test identity: {identity}")
    tests[identity] = status


def _junit(report):
    if not report:
        raise EvalError("Missing JUnit report")
    root = ET.fromstring(report)
    tests = {}
    for case in root.iter("testcase"):
        identity = (
            case.get("classname", case.get("file", "")) + "::" + case.attrib["name"]
        )
        if case.find("error") is not None:
            raise SuiteLoadError(f"Test setup/collection error: {identity}")
        status = (
            "failed"
            if case.find("failure") is not None
            else "skipped"
            if case.find("skipped") is not None
            else "passed"
        )
        _insert(tests, identity, status)
    if any(int(s.get("errors", "0")) for s in root.iter("testsuite")):
        raise SuiteLoadError("JUnit suite reports collection/setup errors")
    return tests


def _jest(report):
    if not report:
        raise EvalError("Missing Jest/Vitest JSON report")
    data = json.loads(report)
    tests = {}
    if data.get("wasInterrupted"):
        raise EvalError("Interrupted test run")
    if data.get("numRuntimeErrorTestSuites", 0):
        raise SuiteLoadError("Suite runtime error")
    for suite in data["testResults"]:
        assertions = suite.get("assertionResults", [])
        if suite.get("status") == "failed" and not assertions:
            raise SuiteLoadError("Test suite failed before collecting test cases")
        name = suite["name"].removeprefix("/work/")
        counts = Counter(case["fullName"] for case in assertions)
        occurrences = Counter()
        for case in assertions:
            raw = case["status"]
            if raw not in {
                "passed",
                "failed",
                "pending",
                "todo",
                "skipped",
                "disabled",
            }:
                raise EvalError(f"Unknown assertion status: {raw}")
            title = case["fullName"]
            occurrences[title] += 1
            # Immutable test files can contain repeated parameterized titles.
            # Reporter declaration order disambiguates these; the gate checks stability.
            suffix = f"#occurrence={occurrences[title]}" if counts[title] > 1 else ""
            _insert(
                tests,
                name + "::" + title + suffix,
                raw if raw in {"passed", "failed"} else "skipped",
            )
    if data.get("numTotalTests") is not None and data["numTotalTests"] != len(tests):
        raise EvalError("JSON test count disagrees with collected identities")
    return tests


def _go(output):
    tests = {}
    completed_packages = set()
    packages = set()
    for line in output.splitlines():
        if not line.startswith("{"):
            continue
        event = json.loads(line)
        if "Package" not in event:
            continue
        package = event["Package"]
        packages.add(package)
        action = event.get("Action")
        if event.get("Test") and action in {"pass", "fail", "skip"}:
            _insert(
                tests,
                package + "::" + event["Test"],
                {"pass": "passed", "fail": "failed", "skip": "skipped"}[action],
            )
        elif not event.get("Test") and action in {"pass", "fail", "skip"}:
            completed_packages.add(package)
            if action == "fail" and not any(
                k.startswith(package + "::") and v == "failed" for k, v in tests.items()
            ):
                raise SuiteLoadError(f"Go package build/setup failure: {package}")
    if packages != completed_packages:
        raise EvalError("Go report has incomplete packages")
    return tests


def _cargo(output):
    tests = {}
    binary = ""
    for line in map(str.strip, output.splitlines()):
        if line.startswith("Running "):
            match = re.search(r"\(([^)]+)\)", line)
            name = (match.group(1) if match else line[8:]).split("/")[-1]
            binary = re.sub(r"-[a-fA-F0-9]+$", "", name)
        elif line.startswith("Doc-tests "):
            binary = "doc:" + line[10:].strip()
        else:
            match = re.fullmatch(
                r"test (.+) \.\.\. (ok|FAILED|ignored)(?:[ ,].*)?", line
            )
            if match:
                if not binary:
                    raise EvalError("Cargo result has no test binary identity")
                _insert(
                    tests,
                    binary + "::" + match[1],
                    {"ok": "passed", "FAILED": "failed", "ignored": "skipped"}[
                        match[2]
                    ],
                )
    if "could not compile" in output or "error: doctest failed" in output and not tests:
        raise SuiteLoadError("Cargo build failure")
    if tests and "test result:" not in output:
        raise EvalError("Incomplete Cargo results")
    return tests


def normalize(plan, exit_code, duration, report, output, artifact):
    try:
        tests = parse_results(plan.result_format, report, output)
        if not tests:
            raise EvalError("Zero collected tests")
        failures = any(s == "failed" for s in tests.values())
        if exit_code not in ({0, 1, 101} if plan.framework == "cargo" else {0, 1}):
            raise EvalError(f"Runner exited {exit_code}")
        if (exit_code == 0) == failures:
            raise EvalError("Exit status and per-test report disagree")
        return TestRun(
            "test_failure" if failures else "passed",
            tests,
            exit_code,
            duration,
            artifact=artifact,
        )
    except SuiteLoadError as error:
        return TestRun("suite_error", {}, exit_code, duration, str(error), artifact)
    except EvalError as error:
        return TestRun(
            "infrastructure_error", {}, exit_code, duration, str(error), artifact
        )


def verify_gate(before, after):
    if len(before) != 2 or len(after) != 2:
        raise EvalError("Validation requires two clean before and after runs")
    for run in before + after:
        if run.status not in {"passed", "test_failure"}:
            raise EvalError(f"Validation infrastructure failure: {run.reason}")
    if before[0].tests != before[1].tests or after[0].tests != after[1].tests:
        raise EvalError("Flaky tests: results changed across identical clean runs")
    if set(before[0].tests) != set(after[0].tests):
        raise EvalError("Old and reference code collected different test identities")
    if any(run.status != "passed" for run in after):
        raise EvalError("Reference fix does not pass the selected suite")
    required = sorted(
        k
        for k, v in before[0].tests.items()
        if v == "failed" and after[0].tests.get(k) == "passed"
    )
    if not required:
        raise EvalError("No repeatable fail-to-pass regression tests")
    existing = sorted(
        k
        for k, v in before[0].tests.items()
        if v == "passed" and after[0].tests.get(k) == "passed"
    )
    expected = sorted(k for k, v in after[0].tests.items() if v == "passed")
    return {
        "fail_to_pass": required,
        "pass_to_pass": existing,
        "expected_tests": expected,
        "skipped_tests": sorted(k for k, v in after[0].tests.items() if v == "skipped"),
    }


def grade(run, validation):
    if run.status == "suite_error":
        return {
            "outcome": "unresolved",
            "failure_reason": "Candidate code prevented the verified suite from compiling/loading: "
            + str(run.reason),
        }
    if run.status not in {"passed", "test_failure"}:
        return {"outcome": "infrastructure_error", "failure_reason": run.reason}
    missing = [k for k in validation["expected_tests"] if k not in run.tests]
    if missing:
        return {
            "outcome": "invalid_result",
            "failure_reason": f"Missing expected tests: {missing}",
        }
    not_passed = [k for k in validation["expected_tests"] if run.tests[k] != "passed"]
    if not_passed or run.status != "passed":
        return {
            "outcome": "unresolved",
            "failure_reason": f"Required or existing tests did not pass: {not_passed}",
        }
    return {"outcome": "resolved", "failure_reason": None}
