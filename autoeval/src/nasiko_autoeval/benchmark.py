"""Prospective benchmark membership and sealed evidence, never directory discovery."""

import argparse
import json
import re
import subprocess
from datetime import UTC, datetime
from pathlib import Path

from .integrity import sha256, source_hashes, verify_checksums
from .report import summarize, write_report
from .results import grade, normalize
from .runner import GRADING_VERSION, DockerRunner
from .schema import EvalError, ExecutionPlan, TestRun, safe_path, save_json
from .tasks import build_task, git

ROOT = Path(__file__).resolve().parents[2]


def _repository(entry, bundle_directory=None):
    name = entry["repository_cache"]
    if not re.fullmatch(r"[A-Za-z0-9_-]+", name):
        raise EvalError("Repository cache name must be a simple identifier")
    repository = ROOT / ".cache" / "benchmark-repos" / name
    if not (repository / ".git").exists():
        repository.parent.mkdir(parents=True, exist_ok=True)
        source = (
            str(Path(bundle_directory) / entry["id"] / "source.bundle")
            if bundle_directory
            else entry["repository_url"]
        )
        result = subprocess.run(
            ["git", "clone", "--quiet", "--no-checkout", source, str(repository)],
            capture_output=True,
            check=False,
            timeout=120,
        )
        if result.returncode:
            raise EvalError(
                "Repository clone failed: "
                + result.stderr.decode(errors="replace")[:500]
            )
        if bundle_directory:
            git(repository, "remote", "set-url", "origin", entry["repository_url"])
    if (
        git(repository, "remote", "get-url", "origin").decode().strip()
        != entry["repository_url"]
    ):
        raise EvalError("Cached repository origin differs from frozen input")
    return repository


def _prepare_inputs(manifest, manifest_root, directory, bundle_directory):
    entries = []
    identifiers = set()
    for entry in manifest["tasks"]:
        name = entry["id"]
        if not re.fullmatch(r"[A-Za-z0-9_-]+", name) or name in identifiers:
            raise EvalError("Benchmark task IDs must be unique simple identifiers")
        identifiers.add(name)
        repository = _repository(entry, bundle_directory)
        destination = directory / "inputs" / name
        destination.mkdir(parents=True)
        issue = (manifest_root / safe_path(entry["issue_file"])).read_text()
        overrides = (
            json.loads((manifest_root / safe_path(entry["config_file"])).read_text())
            if entry.get("config_file")
            else {}
        )
        task, plan, *_ = build_task(
            repository,
            entry["commit"],
            destination,
            entry.get("unit"),
            overrides,
            issue,
        )
        if task["input_kind"] != "real_repository":
            raise EvalError("Synthetic tasks cannot enter the real benchmark")
        with DockerRunner(directory / ".preparation") as runner:
            image = runner.resolve_image(plan)
        digest = (image.get("RepoDigests") or [image["Id"]])[0]
        overrides["runtime_image"] = digest
        save_json(destination / "execution.json", overrides)
        (destination / "issue.txt").write_text(issue)
        git(
            repository,
            "bundle",
            "create",
            str((destination / "source.bundle").resolve()),
            "--all",
        )
        entries.append(
            {
                **entry,
                "parent_sha": task["parent_sha"],
                "fix_sha": task["fix_sha"],
                "repository_path": str(repository),
                "runtime_tag": plan.runtime_image,
                "runtime_image_id": image["Id"],
                "runtime_digest": digest,
                "run": "runs/" + name,
                "issue_sha256": sha256(destination / "issue.txt"),
                "configuration_sha256": sha256(destination / "execution.json"),
                "source_bundle_sha256": sha256(destination / "source.bundle"),
            }
        )
    return entries


def run_benchmark(args):
    from .workflow import run_workflow

    source = Path(args.manifest).resolve()
    manifest = json.loads(source.read_text())
    if manifest.get("kind") != "real_repository_benchmark" or not manifest.get("tasks"):
        raise EvalError("Expected an explicit real_repository_benchmark manifest")
    models = manifest["models"]
    if len(models) != len(set(models)) or not all(
        "\x00" not in model and ":" in model for model in models
    ):
        raise EvalError("Frozen models must be unique provider:model selectors")
    directory = Path(args.run).resolve()
    if directory.exists() and any(directory.iterdir()):
        raise EvalError(
            "Benchmark output must be a fresh directory; sealed runs cannot be resumed/overwritten"
        )
    directory.mkdir(parents=True, exist_ok=True)
    entries = _prepare_inputs(manifest, source.parent, directory, args.source_bundles)
    planned = {
        "schema_version": 1,
        "name": manifest["name"],
        "kind": manifest["kind"],
        "created_at": datetime.now(UTC).isoformat(),
        "manifest_sha256": sha256(source),
        "models": models,
        "tasks": entries,
        "policy": manifest["selection_policy"],
        "generation_attempts_per_task_model": 1,
        "source_sha256": source_hashes(),
        "grading_version": GRADING_VERSION,
        "tamper_proof": False,
        "preflight_attempts": manifest.get("preflight_attempts", 1),
    }
    save_json(directory / "planned.json", planned)
    replay_manifest = json.loads(json.dumps(manifest))
    for entry in replay_manifest["tasks"]:
        entry["issue_file"] = "inputs/" + entry["id"] + "/issue.txt"
        entry["config_file"] = "inputs/" + entry["id"] + "/execution.json"
    save_json(directory / "manifest.json", replay_manifest)
    for entry in entries:
        if source_hashes() != planned["source_sha256"]:
            raise EvalError("Evaluator source changed after benchmark planning")
        destination = directory / entry["run"]
        controls = directory / "inputs" / entry["id"]
        command = argparse.Namespace(
            command="compare",
            repo=Path(entry["repository_path"]),
            commit=entry["fix_sha"],
            unit=entry.get("unit"),
            config=controls / "execution.json",
            issue_file=controls / "issue.txt",
            run=destination,
            model=models,
            max_output_tokens=entry["max_output_tokens"],
            context_bytes=manifest["context_bytes"],
            allow_cost_gaps=False,
            preflight_attempts=manifest.get("preflight_attempts", 1),
            server_url=args.server_url,
            env_file=args.env_file,
            postgres_container=args.postgres_container,
        )
        run_workflow(command)
        result = json.loads((destination / "results.json").read_text())
        if result.get("validation", {}).get("status") != "verified":
            raise EvalError(
                f"Final task {entry['id']} failed its gate; preserve this run and investigate without substituting tasks"
            )
        for selector in models:
            model = selector.split(":", 1)[1]
            matching = [a for a in result["attempts"] if a["model"] == model]
            if len(matching) > 1:
                raise EvalError(
                    "More than one generation attempt in a frozen task/model slot"
                )
            if not matching:
                result["attempts"].append(
                    {
                        "attempt_id": result["run_id"] + ":unavailable-" + model,
                        "task": entry["fix_sha"],
                        "language": result["task"]["plan"]["language"],
                        "framework": result["task"]["plan"]["framework"],
                        "model": model,
                        "generation_attempted": False,
                        "outcome": "model_error",
                        "cost_usd": None,
                        "failure_reason": "Availability/exact-price preflight failed; assigned slot retained in denominator",
                        "usage": None,
                        "duration_seconds": 0,
                    }
                )
        _portable_artifacts(result, destination)
        save_json(destination / "validation.json", result["validation"])
        write_report(destination, result)
    if source_hashes() != planned["source_sha256"]:
        raise EvalError("Evaluator source changed during the benchmark")
    results = collect_members(directory, planned)
    save_json(directory / "results.json", results)
    render_benchmark(directory, results)
    checksums = {
        str(path.relative_to(directory)): sha256(path)
        for path in sorted(directory.rglob("*"))
        if path.is_file() and ".preparation" not in path.parts
    }
    save_json(
        directory / "benchmark.lock.json",
        {
            "status": "sealed",
            "planned_sha256": sha256(directory / "planned.json"),
            "members": [entry["run"] for entry in entries],
            "checksums": checksums,
        },
    )
    verify_benchmark(directory)
    print(f"SEALED final benchmark: {directory / 'report.md'}", flush=True)
    return 0


def _portable_artifacts(value, run_directory):
    if isinstance(value, dict):
        for key, item in value.items():
            if key == "artifact" and isinstance(item, str):
                try:
                    value[key] = str(Path(item).relative_to(run_directory))
                except ValueError:
                    raise EvalError("Artifact escaped its run directory") from None
            else:
                _portable_artifacts(item, run_directory)
    elif isinstance(value, list):
        for item in value:
            _portable_artifacts(item, run_directory)


def collect_members(directory, planned):
    directory = Path(directory)
    attempts, verified, preflight, failures = [], [], [], []
    for entry in planned["tasks"]:
        run = json.loads(
            (directory / safe_path(entry["run"]) / "results.json").read_text()
        )
        task = run["task"]
        if (
            task.get("input_kind") != "real_repository"
            or task["fix_sha"] != entry["fix_sha"]
            or task["parent_sha"] != entry["parent_sha"]
            or task["repository_url"] != entry["repository_url"]
        ):
            raise EvalError("Frozen membership includes a synthetic or mismatched task")
        expected = {model.split(":", 1)[1] for model in planned["models"]}
        if (
            len(run["attempts"]) != len(expected)
            or {a["model"] for a in run["attempts"]} != expected
        ):
            raise EvalError(
                "Frozen benchmark has missing/duplicate/unplanned model slots"
            )
        verified.append(
            {
                "task": task,
                "validation": run["validation"],
                "source": entry["run"] + "/results.json",
            }
        )
        attempts.extend(
            {**a, "source_run": entry["run"], "budget": run.get("budget")}
            for a in run["attempts"]
        )
        preflight.extend(run.get("preflight", []))
        failures.extend(run.get("preflight_failures", []))
    return {
        "schema_version": 2,
        "run_kind": "frozen_benchmark",
        "run_id": planned["name"],
        "verified_tasks": verified,
        "attempts": attempts,
        "summary": summarize(attempts),
        "preflight": preflight,
        "preflight_failures": failures,
        "tamper_proof": False,
        "grading_version": planned["grading_version"],
        "membership": [entry["run"] for entry in planned["tasks"]],
    }


def render_benchmark(directory, results):
    from .report import _money

    lines = [
        "# Nasiko AutoEval — frozen benchmark",
        "",
        f"Dataset: `{results['run_id']}`. {len(results['verified_tasks'])} real tasks; {len(results['attempts'])} assigned model/task slots.",
        "",
        "Membership was declared before final generation in planned.json. Only those runs are scored. Synthetic calculator demos, security probes, pricing diagnostics and development reruns are excluded.",
        "",
        "| Model | Resolved / assigned | Rate | Infrastructure failures | Generation cost | Cost / assigned task | Cost / resolution | Cost coverage |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in results["summary"]:
        lines.append(
            f"| {row['model']} | {row['resolved']} / {row['attempted']} | {row['resolution_rate']:.1%} | {row['infrastructure_failures']} | {_money(row['cost_usd'])} | {_money(row['cost_per_attempt_usd'])} | {_money(row['cost_per_resolution_usd'])} | {row['cost_coverage']} |"
        )
    lines += [
        "",
        "Costs are Nasiko ledger values at Exact, non-estimated rates; preflight is separate overhead. No local token-price estimate replaces a recorded amount.",
        "",
        "| Task | Language / framework | Regression requirements | Existing passing requirements |",
        "| --- | --- | ---: | ---: |",
    ]
    for item in results["verified_tasks"]:
        task, gate = item["task"], item["validation"]
        lines.append(
            f"| `{task['fix_sha']}` | {task['plan']['language']} / {task['plan']['framework']} | {len(gate['fail_to_pass'])} | {len(gate['pass_to_pass'])} |"
        )
    lines += ["", "## Outcomes", ""]
    for attempt in results["attempts"]:
        reason = attempt.get("failure_reason") or "All expected tests passed"
        if len(reason) > 350:
            reason = reason[:347] + "... (full identities in results.json)"
        lines.append(
            f"- `{attempt['task'][:12]}` / {attempt['model']}: **{attempt['outcome']}**, {_money(attempt.get('cost_usd'))}. {reason}"
        )
    known = sum(
        p["cost_usd"] for p in results["preflight"] if p.get("cost_usd") is not None
    )
    lines += [
        "",
        f"Preflight overhead: {_money(known)} known recorded; {len(results['preflight_failures'])} failed/retried probes. Failed calls with no provider usage have unknown cost and are not silently treated as free.",
        "",
        "## Trust and reproducibility",
        "",
        "Reports/logs are captured outside the candidate container. The report/log overwrite plus forced-exit regression was reproduced before hardening and rejected afterward. Candidate code still runs inside test frameworks and may manipulate framework state or forge process output: **grading is not tamper-proof**. These are observed test outcomes for ordinary one-shot patches, not adversarially secure correctness proofs.",
        "",
        "The sealed file checksums, pinned runtime digests, dependency inventories, repository Git bundles, exact SHAs, immutable test identities, prompts, candidates, raw captures and Nasiko usage records are retained. Public-task selection bias and model training overlap remain uncontrolled; no general model ranking follows from this sample.",
        "",
    ]
    (Path(directory) / "report.md").write_text("\n".join(lines))


def verify_benchmark(directory):
    directory = Path(directory)
    lock = json.loads((directory / "benchmark.lock.json").read_text())
    if lock.get("status") != "sealed":
        raise EvalError("Benchmark is not sealed")
    verify_checksums(directory, lock["checksums"])
    planned = json.loads((directory / "planned.json").read_text())
    if source_hashes() != planned["source_sha256"]:
        raise EvalError(
            "Evaluator code differs from the source recorded in this benchmark"
        )
    if sha256(directory / "planned.json") != lock["planned_sha256"] or lock[
        "members"
    ] != [entry["run"] for entry in planned["tasks"]]:
        raise EvalError("Frozen benchmark membership changed")
    computed = collect_members(directory, planned)
    saved = json.loads((directory / "results.json").read_text())
    if computed != saved:
        raise EvalError(
            "Saved headline metrics differ from the explicitly selected evidence"
        )
    for entry in planned["tasks"]:
        _verify_captures(directory / entry["run"])
    return saved


def _verify_captures(run_directory):
    run = json.loads((run_directory / "results.json").read_text())
    plan = ExecutionPlan(**run["task"]["plan"])
    runs = run["validation"]["before"] + run["validation"]["reference"]
    runs += [a["test_run"] for a in run["attempts"] if a.get("test_run")]
    for saved in runs:
        artifact = run_directory / safe_path(saved["artifact"])
        execution = json.loads((artifact / "execution.json").read_text())
        limits = execution["limits"]
        if (
            execution["grading_version"] != GRADING_VERSION
            or execution["user"] != "10001:10001"
            or limits["NetworkMode"] != "none"
            or not limits["ReadonlyRootfs"]
            or "/eval/out" in limits.get("Tmpfs", {})
        ):
            raise EvalError("Captured execution does not match the hardened boundary")
        if (
            saved["status"] in {"timeout", "output_limit", "infrastructure_error"}
            and "observed_exit_code" not in execution
        ):
            continue
        stdout = (artifact / "stdout.log").read_text()
        decoded = normalize(
            plan,
            execution["observed_exit_code"],
            saved["duration_seconds"],
            stdout if plan.result_format in {"jest", "junit"} else None,
            stdout,
            saved["artifact"],
            protocol_token=execution.get("protocol_token"),
        )
        if decoded.to_dict() != saved:
            raise EvalError("Saved per-test results disagree with the raw host capture")
    for attempt in run["attempts"]:
        if attempt.get("test_run"):
            verdict = grade(TestRun(**attempt["test_run"]), run["validation"])
            if verdict["outcome"] != attempt["outcome"]:
                raise EvalError(
                    "Saved resolution differs from regrading the captured test results"
                )
