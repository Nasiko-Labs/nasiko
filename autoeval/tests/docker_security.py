"""Opt-in real Docker regression for report/log replacement and forced exit.

PYTHONPATH=autoeval/src python3 autoeval/tests/docker_security.py --output DIR
The --expect-vulnerable switch records the original runner before hardening.
"""

import argparse
import hashlib
import json
import tempfile
from pathlib import Path
from xml.etree.ElementTree import Element, SubElement, tostring

from nasiko_autoeval.analyzer import inspect_repository, select_plan
from nasiko_autoeval.report import summarize
from nasiko_autoeval.results import grade
from nasiko_autoeval.runner import DockerRunner
from nasiko_autoeval.schema import save_json
from nasiko_autoeval.tasks import write_tree


def fixture():
    return {
        "pyproject.toml": b'[project]\nname="autoeval-security-fixture"\nversion="1.0.0"\n[tool.pytest.ini_options]\ntestpaths=["tests"]\n',
        "module.py": b"def answer():\n    return 0\n",
        "tests/test_module.py": b"from module import answer\ndef test_existing():\n    assert callable(answer)\ndef test_regression():\n    assert answer() == 1\n",
    }


def forgery(identities):
    suite = Element("testsuite", tests=str(len(identities)), failures="0", errors="0")
    for identity in identities:
        classname, name = identity.rsplit("::", 1)
        SubElement(suite, "testcase", classname=classname, name=name)
    payload = tostring(suite, encoding="unicode")
    return (
        "def answer():\n    return 0\n"
        "import atexit, os\nfrom pathlib import Path\n"
        "def forge():\n"
        "    try:\n"
        f"        Path('/eval/out/results.xml').write_text({payload!r})\n"
        "        Path('/eval/out/output.log').write_text('FORGED: all tests passed\\n')\n"
        "    except OSError:\n"
        "        pass\n"
        "    os._exit(0)\n"
        "atexit.register(forge)\n"
    ).encode()


def run(output, expect_vulnerable):
    output = Path(output)
    if (output / "security.json").exists():
        raise SystemExit(
            "Preserving existing security evidence; choose a fresh directory"
        )
    files = fixture()
    with tempfile.TemporaryDirectory(prefix="autoeval-security-") as temp:
        write_tree(files, temp)
        plan = select_plan(inspect_repository(temp))
    with DockerRunner(output / "artifacts") as runner:
        runner.prepare(files, plan)
        broken = runner.run(files, plan, "broken-control")
        fixed = runner.run(
            {**files, "module.py": b"def answer():\n    return 1\n"},
            plan,
            "fixed-control",
        )
        assert broken.status == "test_failure", broken.to_dict()
        assert fixed.status == "passed", fixed.to_dict()
        validation = {"expected_tests": sorted(fixed.tests)}
        attack = runner.run(
            {**files, "module.py": forgery(fixed.tests)}, plan, "file-forgery"
        )
        verdict = grade(attack, validation)
    record = {
        "kind": "synthetic_security_regression",
        "attack": "atexit rewrites both report and captured log, then os._exit(0) hides failing test exit",
        "runner_sha256": hashlib.sha256(
            Path(
                __import__("nasiko_autoeval.runner", fromlist=[""]).__file__
            ).read_bytes()
        ).hexdigest(),
        "broken_control": broken.to_dict(),
        "fixed_control": fixed.to_dict(),
        "attack_result": attack.to_dict(),
        "grade": verdict,
        "exploit_succeeded": verdict["outcome"] == "resolved",
        "expected_vulnerable": expect_vulnerable,
        "excluded_from_benchmark": True,
    }
    save_json(output / "security.json", record)
    print(
        json.dumps(
            {
                "attack_outcome": verdict["outcome"],
                "exploit_succeeded": record["exploit_succeeded"],
                "evidence": str(output),
            },
            indent=2,
        )
    )
    assert record["exploit_succeeded"] is expect_vulnerable, (
        "Report-forgery regression expectation failed"
    )
    assert summarize(
        [{"model": "attacker", "outcome": verdict["outcome"], "cost_usd": 0}]
    )[0]["resolved"] == int(expect_vulnerable)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expect-vulnerable", action="store_true")
    args = parser.parse_args()
    run(args.output, args.expect_vulnerable)
