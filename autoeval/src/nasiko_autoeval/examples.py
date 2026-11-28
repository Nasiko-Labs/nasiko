"""Synthetic calculator histories; execution always uses the shared workflow."""

import argparse
import hashlib
import json
import os
import subprocess
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from .analyzer import inspect_repository, select_plan
from .schema import EvalError, safe_path, save_json

NAMES = ("python", "javascript", "typescript", "go", "rust")
ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = ROOT / "examples" / "calculators"


def prepare_example(name, cache=None):
    source = EXAMPLES / name
    specification = json.loads((source / "history.json").read_text())
    tracked = {
        path: (source / safe_path(path)).read_bytes() for path in specification["files"]
    }
    history_files = ("history.json", "base-tests.txt", "reference.patch")
    digest = hashlib.sha256()
    for path, data in sorted(
        {
            **tracked,
            **{path: (source / path).read_bytes() for path in history_files},
        }.items()
    ):
        digest.update(path.encode() + b"\0" + data + b"\0")
    key = digest.hexdigest()
    repository = Path(cache or ROOT / ".cache" / "calculators") / (
        name + "-" + key[:12]
    )
    if not (repository / ".git").exists():
        if repository.exists() and any(repository.iterdir()):
            raise EvalError(f"Preserving nonempty example cache: {repository}")
        repository.mkdir(parents=True, exist_ok=True)
        _git(repository, "init", "-q", "--initial-branch=main")
        for path, data in tracked.items():
            target = repository / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        (repository / specification["test_file"]).write_bytes(
            (source / "base-tests.txt").read_bytes()
        )
        _git(repository, "add", ".")
        _git(
            repository,
            "commit",
            "-qm",
            "Synthetic calculator: buggy add and zero-input control",
        )
        _git(repository, "tag", "buggy")
        _git(
            repository,
            "apply",
            "--check",
            "--whitespace=nowarn",
            str(source / "reference.patch"),
        )
        _git(
            repository, "apply", "--whitespace=nowarn", str(source / "reference.patch")
        )
        (repository / specification["test_file"]).write_bytes(
            tracked[specification["test_file"]]
        )
        _git(repository, "add", ".")
        _git(
            repository,
            "commit",
            "-qm",
            "Synthetic fix: add both operands; cover positive and negative sums",
        )
        _git(repository, "tag", "reference-fix")
    if _git(repository, "status", "--porcelain").strip():
        raise EvalError(f"Example cache has local edits: {repository}")
    commit = _git(repository, "rev-parse", "reference-fix").strip()
    parent = _git(repository, "rev-parse", "reference-fix^").strip()
    plan = select_plan(inspect_repository(repository))
    return {
        "name": name,
        "repository": str(repository),
        "parent_sha": parent,
        "fix_sha": commit,
        "fixture_sha256": key,
        "detected_unit": plan.unit,
        "kind": "synthetic_calculator_demo",
    }


def _git(repository, *arguments):
    environment = {
        **os.environ,
        "GIT_AUTHOR_NAME": "AutoEval Synthetic Demo",
        "GIT_AUTHOR_EMAIL": "demo@example.invalid",
        "GIT_COMMITTER_NAME": "AutoEval Synthetic Demo",
        "GIT_COMMITTER_EMAIL": "demo@example.invalid",
        "GIT_AUTHOR_DATE": "2026-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2026-01-01T00:00:00Z",
    }
    result = subprocess.run(
        [
            "git",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-C",
            str(repository),
            *arguments,
        ],
        env=environment,
        capture_output=True,
        text=True,
        check=False,
        timeout=60,
    )
    if result.returncode:
        raise EvalError("Example history creation failed: " + result.stderr[:700])
    return result.stdout


def run_examples(args):
    from .workflow import run_workflow

    selected = NAMES if args.example == "all" else (args.example,)
    stamp = datetime.now(ZoneInfo("Asia/Kolkata")).strftime("%Y%m%d-%H%M%S-%f")
    output = Path(
        args.run or ROOT / "synthetic-runs" / ("calculators-" + stamp)
    ).resolve()
    if (output / "summary.json").exists():
        raise EvalError("Example output already exists; choose a fresh --run")
    rows = []
    for name in selected:
        entry = prepare_example(name)
        print(f"SYNTHETIC {name}: detected {entry['detected_unit']}", flush=True)
        configuration = EXAMPLES / name / "execution.json"
        command = argparse.Namespace(
            command="validate",
            repo=Path(entry["repository"]),
            commit=entry["fix_sha"],
            unit=None,
            config=configuration if configuration.exists() else None,
            issue_file=EXAMPLES / "issue.txt",
            run=output / name,
        )
        code = run_workflow(command)
        result = json.loads((output / name / "results.json").read_text())
        gate = result.get("validation", {})
        rows.append(
            {
                **entry,
                "exit_code": code,
                "verified": gate.get("status") == "verified",
                "fail_to_pass": gate.get("fail_to_pass", []),
                "pass_to_pass": gate.get("pass_to_pass", []),
                "run": name,
                "excluded_from_real_benchmark": True,
            }
        )
        save_json(
            output / "summary.json",
            {
                "kind": "synthetic_calculator_demo",
                "examples": rows,
                "excluded_from_real_benchmark": True,
            },
        )
    (output / "report.md").write_text(
        "# Synthetic calculator demos\n\nExcluded from real repository benchmark scores.\n\n"
        + "\n".join(
            f"- {r['name']}: {'verified' if r['verified'] else 'failed'}; {len(r['fail_to_pass'])} regression / {len(r['pass_to_pass'])} existing tests"
            for r in rows
        )
        + "\n"
    )
    print(f"Synthetic demo report: {output / 'report.md'}", flush=True)
    return 0 if all(row["verified"] for row in rows) else 1
