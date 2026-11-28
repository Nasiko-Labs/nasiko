"""Show recorded real measurements for the three-minute demo, without model calls."""

import json
from pathlib import Path


def main():
    root = Path(__file__).resolve().parent / "results"
    results = json.loads((root / "results.json").read_text())
    print("NASIKO AUTOEVAL — actual recorded repository work\n")
    print(
        f"{len(results['verified_tasks'])} verified real tasks; {len(results['attempts'])} attempts\n"
    )
    for entry in results["verified_tasks"]:
        task, gate = entry["task"], entry["validation"]
        print(
            f"{task['plan']['language']:12} {task['fix_sha'][:12]}: "
            f"{len(gate['fail_to_pass'])} regression + {len(gate['pass_to_pass'])} existing tests; twice before/after"
        )
    print()
    for row in results["summary"]:
        print(
            f"{row['model']:25} {row['resolved']}/{row['attempted']} resolved "
            f"({row['resolution_rate']:.1%}); infrastructure failures={row['infrastructure_failures']}"
        )
    print(
        "\nVerified cost / attempt and cost / resolution: N/A (estimated Nasiko rates rejected)."
    )
    print(
        "Actual recorded ledger amounts, tokens and trace IDs are in results.json / report.md."
    )
    print(
        "Synthetic fixtures are reported separately from real repository measurements."
    )
    print(f"\nEvidence: {root / 'report.md'}")


if __name__ == "__main__":
    main()
