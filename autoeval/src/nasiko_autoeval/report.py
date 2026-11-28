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
    """Read a sealed, explicitly enumerated benchmark; never glob development runs."""
    from .benchmark import verify_benchmark

    return verify_benchmark(directory)
