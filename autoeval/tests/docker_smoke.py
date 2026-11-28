"""Explicit Docker integration checks using tiny synthetic fixtures, never benchmark data.

PYTHONPATH=autoeval/src python3 autoeval/tests/docker_smoke.py [pytest jest vitest go cargo]
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

from nasiko_autoeval.cli import main
from nasiko_autoeval.schema import save_json

FIXTURES = {
    "pytest": {
        "pyproject.toml": '[project]\nname="autoeval-fixture"\nversion="1.0.0"\n[tool.pytest.ini_options]\ntestpaths=["tests"]\n',
        "module.py": "def answer():\n    return 0\n",
        "tests/test_module.py": "from module import answer\ndef test_existing():\n    assert callable(answer)\n",
    },
    "jest": {
        "package.json": json.dumps(
            {
                "name": "autoeval-fixture",
                "private": True,
                "devDependencies": {"jest": "29.7.0"},
            }
        ),
        "module.js": "exports.answer = () => 0;\n",
        "tests/module.test.js": "const {answer} = require('../module');\ntest('existing', () => expect(typeof answer).toBe('function'));\n",
    },
    "vitest": {
        "package.json": json.dumps(
            {
                "name": "autoeval-fixture",
                "private": True,
                "type": "module",
                "devDependencies": {"vitest": "2.1.9", "typescript": "5.7.3"},
            }
        ),
        "module.ts": "export const answer = () => 0;\n",
        "tests/module.test.ts": "import {test, expect} from 'vitest';\nimport {answer} from '../module';\ntest('existing', () => expect(typeof answer).toBe('function'));\n",
    },
    "go": {
        "go.mod": "module example.com/autoeval-fixture\n\ngo 1.21\n",
        "module.go": "package fixture\nfunc Answer() int { return 0 }\n",
        "module_test.go": 'package fixture\nimport "testing"\nfunc TestExisting(t *testing.T) { if Answer() < 0 { t.Fatal("negative") } }\n',
    },
    "cargo": {
        "Cargo.toml": '[package]\nname="autoeval-fixture"\nversion="0.1.0"\nedition="2021"\n',
        "src/lib.rs": "pub fn answer() -> i32 { 0 }\n",
        "tests/regression.rs": "#[test]\nfn existing() { assert!(autoeval_fixture::answer() >= 0); }\n",
    },
}


def git(root, *args):
    result = subprocess.run(
        ["git", "-C", str(root), *args], capture_output=True, text=True, check=True
    )
    return result.stdout.strip()


def run_fixture(name, destination):
    files = FIXTURES[name]
    with tempfile.TemporaryDirectory(prefix="autoeval-fixture-") as temp:
        root = Path(temp)
        for path, content in files.items():
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
        git(root, "init", "-q")
        git(root, "config", "user.email", "fixture@example.invalid")
        git(root, "config", "user.name", "AutoEval Fixture")
        git(root, "add", ".")
        git(root, "commit", "-qm", "Synthetic fixture parent")
        source = next(
            path
            for path in files
            if path.startswith(("module.", "src/lib")) and "_test" not in path
        )
        (root / source).write_text(
            files[source]
            .replace("return 0", "return 1")
            .replace("=> 0", "=> 1")
            .replace("{ 0 }", "{ 1 }")
        )
        test = next(
            path for path in files if "test" in path and path != "pyproject.toml"
        )
        regression = {
            "pytest": "\ndef test_regression():\n    assert answer() == 1\n",
            "jest": "test('regression', () => expect(answer()).toBe(1));\n",
            "vitest": "test('regression', () => expect(answer()).toBe(1));\n",
            "go": '\nfunc TestRegression(t *testing.T) { if Answer() != 1 { t.Fatal("wrong answer") } }\n',
            "cargo": "\n#[test]\nfn regression() { assert_eq!(autoeval_fixture::answer(), 1); }\n",
        }[name]
        (root / test).write_text(files[test] + regression)
        git(root, "add", ".")
        git(root, "commit", "-qm", "Synthetic regression fix")
        command = [
            "validate",
            "--repo",
            str(root),
            "--commit",
            "HEAD",
            "--run",
            str(destination),
        ]
        if name == "cargo":
            override = root / "override.json"
            override.write_text(json.dumps({"runtime_image": "rust:1-slim-bookworm"}))
            command.extend(["--config", str(override)])
        code = main(command)
        results = json.loads((destination / "results.json").read_text())
        return {
            "fixture": name,
            "kind": "synthetic_adapter_verification",
            "exit_code": code,
            "verified": results.get("validation", {}).get("status") == "verified",
            "run": str(destination),
        }


if __name__ == "__main__":
    import os

    base = Path(
        os.environ.get("AUTOEVAL_SMOKE_OUTPUT", "autoeval/results/adapter-smoke")
    )
    selected = sys.argv[1:] or list(FIXTURES)
    rows = []
    for fixture in selected:
        directory = base / fixture
        if (directory / "results.json").exists():
            raise SystemExit(
                f"Preserving existing evidence: select a fresh output directory by editing base: {directory}"
            )
        rows.append(run_fixture(fixture, directory))
        save_json(base / "summary.json", rows)
    raise SystemExit(0 if all(row["verified"] for row in rows) else 1)
