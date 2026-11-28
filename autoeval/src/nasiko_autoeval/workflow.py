"""The shared task-to-report workflow. Toolchains only supply execution plans."""

import hashlib
import json
import time
import uuid
from datetime import datetime
from pathlib import Path
from zoneinfo import ZoneInfo

from .nasiko import NasikoClient
from .report import write_report
from .results import grade, verify_gate
from .runner import GRADING_VERSION, DockerRunner
from .schema import EvalError, save_json
from .tasks import apply_candidate, build_task, git, model_prompt


def run_workflow(args):
    if args.command == "compare" and (
        args.context_bytes < 1 or args.max_output_tokens < 1
    ):
        raise EvalError("Model context and output budgets must be positive")
    run_id = (
        "autoeval-"
        + datetime.now(ZoneInfo("Asia/Kolkata")).strftime("%Y%m%d-%H%M%S")
        + "-"
        + uuid.uuid4().hex[:6]
    )
    directory = Path(
        args.run or Path(__file__).resolve().parents[2] / "runs" / run_id
    ).resolve()
    if (directory / "results.json").exists():
        raise EvalError(
            f"Run already exists; select a new --run directory: {directory}"
        )
    directory.mkdir(parents=True, exist_ok=True)
    overrides = json.loads(Path(args.config).read_text()) if args.config else None
    issue = Path(args.issue_file).read_text() if args.issue_file else None
    results = {
        "schema_version": 1,
        "run_id": run_id,
        "mode": "one_shot",
        "attempts": [],
        "preflight": [],
        "preflight_failures": [],
        "blockers": [],
        "grading": {
            "version": GRADING_VERSION,
            "host_owned_capture": True,
            "tamper_proof": False,
            "scope": "File/log replacement regression verified; in-process framework/stdout manipulation remains a risk",
        },
    }
    try:
        task, plan, base, before, reference = build_task(
            args.repo, args.commit, directory, args.unit, overrides, issue
        )
        results["task"] = task
        parent = git(args.repo, "rev-parse", task["fix_sha"] + "^").decode().strip()
        task["parent_sha"] = parent
        origin = (
            git(args.repo, "config", "--get", "remote.origin.url").decode().strip()
            if task["input_kind"] == "real_repository"
            else "local fixture"
        )
        from urllib.parse import urlsplit, urlunsplit

        url = urlsplit(origin)
        task["repository_url"] = (
            urlunsplit((url.scheme, url.netloc.rsplit("@", 1)[-1], url.path, "", ""))
            if url.scheme
            else origin
        )
        save_json(directory / "task.json", task)
        write_report(directory, results)
        if args.command == "compare":
            with NasikoClient(
                args.server_url, args.env_file, args.postgres_container
            ) as client:
                configurations = []
                for index, selector in enumerate(args.model):
                    try:
                        configuration = client.resolve_model(selector)
                        if any(c["id"] == configuration["id"] for c in configurations):
                            continue
                        print(f"Preflight: {configuration['name']}", flush=True)
                        probe = _probe(
                            client,
                            configuration,
                            run_id + f":preflight-{index}",
                            args,
                            results,
                        )
                        results["preflight"].append(probe)
                        if probe.get("cost_error"):
                            results["blockers"].append(
                                f"Pricing {selector}: {probe['cost_error']}; correctness-only continuation explicitly enabled"
                            )
                        configurations.append(configuration)
                    except EvalError as error:
                        results["blockers"].append(
                            f"Preflight {selector}: {client.redact(str(error))}"
                        )
                        results["preflight_failures"].append(
                            {
                                "model": selector,
                                "failure_reason": client.redact(str(error)),
                                "cost_usd": None,
                            }
                        )
                    write_report(directory, results)
                _evaluate(
                    task,
                    plan,
                    base,
                    before,
                    reference,
                    directory,
                    results,
                    args,
                    client,
                    configurations,
                )
        else:
            _evaluate(
                task, plan, base, before, reference, directory, results, args, None, []
            )
    except KeyboardInterrupt:
        results["status"] = "interrupted"
        results["blockers"].append(
            "Interrupted by operator; no unfinished work is counted as resolved"
        )
        raise
    except (EvalError, ValueError, TypeError, KeyError) as error:
        results["blockers"].append(str(error))
        results.setdefault("validation", {"status": "rejected", "reason": str(error)})
        write_report(directory, results)
        print(f"Stopped: {error}\nEvidence: {directory}", flush=True)
        return 1
    finally:
        write_report(directory, results)
    print(f"Report: {directory / 'report.md'}", flush=True)
    for row in results["summary"]:
        print(
            f"{row['model']}: {row['resolved']}/{row['attempted']} resolved; cost=${row['cost_usd']:.6f}"
            if row["cost_usd"] is not None
            else f"{row['model']}: {row['resolved']}/{row['attempted']} resolved; cost unavailable"
        )
    return (
        0
        if results.get("validation", {}).get("status") == "verified"
        and (args.command == "validate" or results["attempts"])
        else 1
    )


def _probe(client, configuration, tag, args, results):
    tries = getattr(args, "preflight_attempts", 1)
    if not 1 <= tries <= 3:
        raise EvalError("Preflight attempts must be 1–3")
    for index in range(tries):
        try:
            return client.probe(
                configuration, tag + f"-{index + 1}", args.allow_cost_gaps
            )
        except EvalError as error:
            retryable = any(
                status in str(error) for status in ("HTTP 502", "HTTP 503", "HTTP 429")
            )
            if not retryable or index + 1 == tries:
                raise
            results["preflight_failures"].append(
                {
                    "model": configuration["model"],
                    "failure_reason": client.redact(str(error)),
                    "cost_usd": None,
                    "retried": True,
                }
            )
            print(
                f"Availability preflight retry {index + 2}/{tries}: {configuration['name']}",
                flush=True,
            )
            time.sleep(3)


def _evaluate(
    task,
    plan,
    base,
    before,
    reference,
    directory,
    results,
    args,
    client,
    configurations,
):
    with DockerRunner(directory / "artifacts") as runner:
        print(
            f"Preparing {plan.language}/{plan.framework} dependencies in {plan.runtime_image}",
            flush=True,
        )
        runner.prepare(base, plan)
        results["images"] = {
            "runtime_image_id": runner.base_image_id,
            "prepared_image_id": runner.prepared_image_id,
        }
        old_runs, fixed_runs = [], []
        for repetition in range(2):
            print(
                f"Offline validation {repetition + 1}/2: old code + trusted tests",
                flush=True,
            )
            old_runs.append(runner.run(before, plan, f"before-{repetition + 1}"))
            print(
                f"Offline validation {repetition + 1}/2: reference code + trusted tests",
                flush=True,
            )
            fixed_runs.append(
                runner.run(reference, plan, f"reference-{repetition + 1}")
            )
            results["validation"] = {
                "status": "running",
                "before": [r.to_dict() for r in old_runs],
                "reference": [r.to_dict() for r in fixed_runs],
            }
            write_report(directory, results)
        try:
            gate = verify_gate(old_runs, fixed_runs)
        except EvalError as error:
            results["validation"].update(status="rejected", reason=str(error))
            raise
        results["validation"].update(gate, status="verified", repetitions=2)
        save_json(directory / "validation.json", results["validation"])
        write_report(directory, results)
        print(
            f"VERIFIED: {len(gate['fail_to_pass'])} regression tests; {len(gate['pass_to_pass'])} existing passing tests",
            flush=True,
        )
        if client:
            prompt = model_prompt(task, base, args.context_bytes)
            (directory / "prompt.txt").write_text(prompt)
            results["budget"] = {
                "max_output_tokens": args.max_output_tokens,
                "context_bytes": args.context_bytes,
                "generation_calls_per_model": 1,
                "temperature": 0,
                "prompt_sha256": hashlib.sha256(prompt.encode()).hexdigest(),
            }
            for index, configuration in enumerate(configurations):
                _attempt(
                    task,
                    plan,
                    before,
                    runner,
                    client,
                    configuration,
                    prompt,
                    directory,
                    results,
                    args,
                    index,
                )


def _attempt(
    task,
    plan,
    trusted_tree,
    runner,
    client,
    configuration,
    prompt,
    directory,
    results,
    args,
    index,
):
    tag = results["run_id"] + f":attempt-{index + 1}"
    model = configuration.get("pinned_model") or configuration["model"]
    attempt = {
        "attempt_id": tag,
        "task": task["fix_sha"],
        "language": plan.language,
        "framework": plan.framework,
        "model": model,
        "config": configuration["name"],
        "outcome": "model_error",
        "cost_usd": None,
        "failure_reason": None,
        "usage": None,
    }
    started = time.monotonic()
    print(f"One-shot attempt: {model}", flush=True)
    try:
        record = client.complete(configuration, prompt, tag, args.max_output_tokens)
        save_json(directory / f"model-{index + 1}.json", record)
        attempt.update(
            cost_usd=record["cost_usd"],
            cost_error=record["cost_error"],
            usage=record["usage"],
            generation_seconds=record["duration_seconds"],
        )
        if record["usage"]:
            attempt["nasiko_recorded_cost_usd"] = record["usage"].get("cost_usd")
            attempt["nasiko_cost_verified"] = record["cost_usd"] is not None
        content = (
            record["response"].get("choices", [{}])[0].get("message", {}).get("content")
        )
        if not isinstance(content, str) or not content:
            raise EvalError("Model returned no patch text")
        content = content.strip()
        if content.startswith("```"):
            content = content.split("\n", 1)[1].rsplit("```", 1)[0]
        try:
            candidate = json.loads(content)
        except (ValueError, TypeError):
            attempt["outcome"] = "invalid_patch"
            raise EvalError(
                "Model did not return valid replacement-file JSON"
            ) from None
        if not isinstance(candidate, dict) or set(candidate) != {"files"}:
            attempt["outcome"] = "invalid_patch"
            raise EvalError("Candidate JSON must contain exactly a files array")
        try:
            files = apply_candidate(trusted_tree, candidate.get("files"), task, plan)
        except EvalError:
            attempt["outcome"] = "invalid_patch"
            raise
        save_json(directory / f"candidate-{index + 1}.json", candidate)
        run = runner.run(files, plan, f"attempt-{index + 1}")
        attempt["test_run"] = run.to_dict()
        verdict = grade(run, results["validation"])
        attempt.update(
            outcome=verdict["outcome"], failure_reason=verdict["failure_reason"]
        )
    except EvalError as error:
        attempt["failure_reason"] = client.redact(str(error))
        if attempt.get("cost_usd") is None and not attempt.get("cost_error"):
            attempt["cost_error"] = (
                "Request failed before attributable Nasiko usage could be verified"
            )
    finally:
        attempt["duration_seconds"] = time.monotonic() - started
        results["attempts"].append(attempt)
        write_report(directory, results)
    print(
        f"  {attempt['outcome']}: {attempt.get('failure_reason') or 'all expected tests passed'}",
        flush=True,
    )
