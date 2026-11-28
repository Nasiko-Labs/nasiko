"""Reports distinguish correctness, infrastructure, and missing cost evidence."""

from collections import defaultdict
from pathlib import Path

from .schema import save_json

INFRASTRUCTURE = {"infrastructure_error", "timeout", "model_error", "invalid_result"}


def summarize(attempts):
    groups = defaultdict(list)
    for attempt in attempts:
        groups[attempt["model"]].append(attempt)
    summary = []
    for model, rows in sorted(groups.items()):
        resolved = sum(r["outcome"] == "resolved" for r in rows)
        infrastructure = sum(r["outcome"] in INFRASTRUCTURE for r in rows)
        known = [r["cost_usd"] for r in rows if r.get("cost_usd") is not None]
        total = sum(known)
        complete = len(known) == len(rows)
        summary.append(
            {
                "model": model,
                "attempted": len(rows),
                "resolved": resolved,
                "infrastructure_failures": infrastructure,
                "resolution_rate": resolved / len(rows),
                "known_cost_usd": total,
                "cost_coverage": f"{len(known)}/{len(rows)}",
                "cost_usd": total if complete else None,
                "cost_per_attempt_usd": total / len(rows) if complete else None,
                "cost_per_resolution_usd": total / resolved
                if complete and resolved
                else None,
            }
        )
    return summary


def _money(value):
    return "N/A" if value is None else f"${value:.6f}"


def write_report(run_directory, results):
    run_directory = Path(run_directory)
    results["summary"] = summarize(results.get("attempts", []))
    save_json(run_directory / "results.json", results)
    task = results.get("task", {})
    validation = results.get("validation", {})
    lines = [
        "# Nasiko AutoEval",
        "",
        f"Run: `{results['run_id']}`. One-shot production-file generation through Nasiko; no interactive coding-agent replay.",
        "",
        f"Task kind: **{task.get('input_kind', 'unknown')}**. Repository: {task.get('repository_url', 'local repository')}.",
        f"Parent: `{task.get('parent_sha', '')}`. Reference fix: `{task.get('fix_sha', '')}`.",
        f"Toolchain: {task.get('plan', {}).get('language', '')} / {task.get('plan', {}).get('framework', '')}.",
        "",
        f"Validation: **{validation.get('status', 'not run')}**. "
        + (validation.get("reason") or ""),
        f"Repeated clean runs: {len(validation.get('before', []))} old + {len(validation.get('reference', []))} reference.",
        f"Fail-to-pass: {len(validation.get('fail_to_pass', []))}; pass-to-pass: {len(validation.get('pass_to_pass', []))}.",
        "",
        "| Model | Resolved / attempted | Rate | Infrastructure failures | Attempt cost | Cost / attempted | Cost / resolved |",
        "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for row in results["summary"]:
        lines.append(
            f"| {row['model']} | {row['resolved']} / {row['attempted']} | {row['resolution_rate']:.1%} | {row['infrastructure_failures']} | {_money(row['cost_usd'])} | {_money(row['cost_per_attempt_usd'])} | {_money(row['cost_per_resolution_usd'])} |"
        )
    preflight = results.get("preflight", [])
    overhead_known = sum(
        p["cost_usd"] for p in preflight if p.get("cost_usd") is not None
    )
    overhead = (
        None
        if results.get("preflight_failures")
        or any(p.get("cost_usd") is None for p in preflight)
        else overhead_known
    )
    lines.extend(
        [
            "",
            f"Model preflight overhead (separate from attempts): {_money(overhead)}; known recorded portion: {_money(overhead_known)}.",
            "",
            "Costs are Nasiko's recorded amounts. Missing, ambiguous, or estimated usage stays N/A; no locally estimated prices are substituted.",
            "Infrastructure failures remain in the attempted-task denominator and are also shown separately.",
            "",
            "## Attempt outcomes",
            "",
        ]
    )
    for attempt in results.get("attempts", []):
        lines.append(
            f"- **{attempt['model']}**: {attempt['outcome']}; {attempt.get('duration_seconds', 0):.2f}s; {_money(attempt.get('cost_usd'))}. "
            + (attempt.get("failure_reason") or "All expected tests passed.")
        )
        if attempt.get("cost_error"):
            lines.append("  Cost evidence gap: " + attempt["cost_error"])
            usage = attempt.get("usage") or {}
            if usage.get("cost_usd") is not None:
                lines.append(
                    f"  Nasiko ledger recorded {_money(usage['cost_usd'])} using estimated pricing; excluded from verified-cost metrics. Trace: `{usage.get('trace_id', 'see usage record')}`."
                )
    for blocker in results.get("blockers", []):
        lines.append("- Blocker: " + blocker)
    lines.extend(["", "## Grading evidence", "", "Required regression identities:", ""])
    lines.extend("- `" + test + "`" for test in validation.get("fail_to_pass", []))
    lines.extend(
        [
            "",
            "Exact per-test results, immutable image IDs, plan, prompts, model responses and Nasiko usage provenance are saved beside this report.",
            "",
            "This small public-commit sample measures these attempts only. Public training-data overlap and task-selection bias are uncontrolled; it does not establish a general model ranking.",
            "",
        ]
    )
    (run_directory / "report.md").write_text("\n".join(lines))
    return results


def aggregate_directory(directory):
    """Combine saved evidence, keeping synthetic checks outside benchmark totals."""
    import json

    directory = Path(directory)
    verified = {}
    attempts, preflight, failures, fixtures, sources, rejected = [], [], [], [], [], []
    for path in sorted(directory.rglob("results.json")):
        if path.parent == directory:
            continue
        run = json.loads(path.read_text())
        if run.get("run_kind") == "aggregate":
            continue
        task = run.get("task", {})
        gate = run.get("validation", {})
        if task.get("input_kind") == "local_fixture":
            fixtures.append(
                {
                    "framework": task.get("plan", {}).get("framework"),
                    "status": gate.get("status", "not_run"),
                    "source": str(path.relative_to(directory)),
                }
            )
            continue
        if task.get("input_kind") != "real_repository":
            continue
        sources.append(str(path.relative_to(directory)))
        if gate.get("status") == "verified":
            key = task["repository_url"] + "@" + task["fix_sha"]
            verified[key] = {
                "task": task,
                "validation": gate,
                "source": str(path.relative_to(directory)),
            }
        elif gate.get("status") == "rejected":
            rejected.append(
                {
                    "source": str(path.relative_to(directory)),
                    "reason": gate.get("reason"),
                }
            )
        attempts.extend(
            {
                **attempt,
                "source_run": str(path.relative_to(directory)),
                "budget": run.get("budget"),
            }
            for attempt in run.get("attempts", [])
        )
        preflight.extend(run.get("preflight", []))
        if "preflight_failures" in run:
            failures.extend(run["preflight_failures"])
        else:
            failures.extend(
                {"error": item, "source": str(path.relative_to(directory))}
                for item in run.get("blockers", [])
                if item.startswith("Preflight ")
            )
    results = {
        "schema_version": 1,
        "run_kind": "aggregate",
        "run_id": directory.name,
        "mode": "one_shot",
        "verified_tasks": list(verified.values()),
        "attempts": attempts,
        "preflight": preflight,
        "preflight_failures": failures,
        "synthetic_adapter_checks": fixtures,
        "source_runs": sources,
        "development_validation_rejections": rejected,
        "summary": summarize(attempts),
    }
    save_json(directory / "results.json", results)
    lines = [
        "# Nasiko AutoEval — measured results",
        "",
        "One implementation; repository language and framework are inputs. These are one-shot production-file attempts, not interactive coding-agent replay.",
        "",
        f"**{len(verified)} verified real tasks; {len(attempts)} model attempts.** Synthetic adapter fixtures are excluded from these totals.",
        "",
        "| Repository | Toolchain | Fix commit | Regression tests | Existing tests |",
        "| --- | --- | --- | ---: | ---: |",
    ]
    for item in verified.values():
        task, gate = item["task"], item["validation"]
        lines.append(
            f"| {task['repository_url']} | {task['plan']['language']} / {task['plan']['framework']} | `{task['fix_sha']}` | {len(gate['fail_to_pass'])} | {len(gate['pass_to_pass'])} |"
        )
    lines.extend(
        [
            "",
            "Each accepted task failed on old code and passed on reference code twice in fresh, offline containers. Exact identities and all four results are in its source run.",
            "",
            "| Model | Resolved / attempts | Resolution rate | Infrastructure failures | Cost / attempt | Cost / resolution | Cost coverage |",
            "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
        ]
    )
    for row in results["summary"]:
        lines.append(
            f"| {row['model']} | {row['resolved']} / {row['attempted']} | {row['resolution_rate']:.1%} | {row['infrastructure_failures']} | {_money(row['cost_per_attempt_usd'])} | {_money(row['cost_per_resolution_usd'])} | {row['cost_coverage']} |"
        )
    lines.extend(
        [
            "",
            "Costs require attributable, non-estimated Nasiko ledger records. Missing or estimated rates remain N/A. Preflight probes and their failures are separate from model-attempt denominators.",
            "",
            "## Attempts and limitations",
            "",
        ]
    )
    for attempt in attempts:
        lines.append(
            f"- `{attempt['task'][:12]}` / **{attempt['model']}**: {attempt['outcome']}, {attempt.get('duration_seconds', 0):.2f}s, {_money(attempt.get('cost_usd'))}. "
            + (attempt.get("failure_reason") or "All expected tests passed.")
            + (
                " Cost gap: " + attempt["cost_error"]
                if attempt.get("cost_error")
                else ""
            )
        )
    uncertain = [
        a
        for a in attempts
        if a.get("cost_error") and (a.get("usage") or {}).get("cost_usd") is not None
    ]
    if uncertain:
        lines.extend(
            [
                "",
                "### Recorded ledger amounts with unverified pricing",
                "",
                "These are actual Nasiko records, explicitly marked estimated by Nasiko. They are audit evidence, not verified spend or a basis for a cost ranking.",
                "",
                "| Task | Model | Nasiko recorded (estimated USD) | Trace |",
                "| --- | --- | ---: | --- |",
            ]
        )
        for attempt in uncertain:
            usage = attempt["usage"]
            lines.append(
                f"| `{attempt['task'][:12]}` | {attempt['model']} | {_money(usage['cost_usd'])} | `{usage.get('trace_id', 'see usage record')}` |"
            )
    lines.extend(
        [
            "",
            (
                f"Preflight: {len(preflight)} text responses; {len(failures)} unavailable/error results. "
                "Older prebuilt servers use the documented read-only Postgres ledger bridge because /api/evals/* is absent."
            ),
            "",
            "Verified adapter fixture runs (synthetic, excluded from benchmark): "
            + ", ".join(
                sorted({f["framework"] for f in fixtures if f["status"] == "verified"})
            ),
            "",
            "This is a small, selected sample of public commits. Training-data overlap, independent issue review and broad repository compatibility are unmeasured. A model ranking is not supported.",
            "",
            "Reproduction commands and grading boundaries: [README](../README.md).",
            "",
        ]
    )
    (directory / "report.md").write_text("\n".join(lines))
    return results
