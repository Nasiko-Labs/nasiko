"""Display the verified frozen real benchmark without making model requests."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "src"))
from nasiko_autoeval.benchmark import verify_benchmark


def money(value):
    return "N/A" if value is None else f"${value:.6f}"


def main():
    root = Path(__file__).resolve().parent / "final"
    result = verify_benchmark(root)
    print("NASIKO AUTOEVAL — frozen real benchmark")
    print("Verified recorded model runs; this display makes no model requests.")
    print(
        f"{len(result['verified_tasks'])} real tasks; {len(result['attempts'])} assigned model/task slots"
    )
    for item in result["verified_tasks"]:
        task, gate = item["task"], item["validation"]
        print(
            f"  {task['plan']['language']}: {len(gate['fail_to_pass'])} regression / {len(gate['pass_to_pass'])} existing requirements; repeated twice"
        )
    print()
    for row in result["summary"]:
        print(
            f"{row['model']}: {row['resolved']}/{row['attempted']} ({row['resolution_rate']:.1%}); infrastructure={row['infrastructure_failures']}"
        )
        print(
            f"  Nasiko generation cost={money(row['cost_usd'])}; per assigned task={money(row['cost_per_attempt_usd'])}; per resolution={money(row['cost_per_resolution_usd'])}"
        )
    print(
        "\nScores are regraded from captured test evidence; model names only group results."
    )
    print("Synthetic calculator validation uses no model calls and is excluded.")
    print(
        "Only declared final runs are scored; security fixtures and development reruns are excluded."
    )
    print(
        "Report/log overwrite regression blocked; in-process framework/stdout manipulation remains a risk. Grading is not tamper-proof."
    )
    print(f"Evidence: {root / 'report.md'}")


if __name__ == "__main__":
    main()
