#!/usr/bin/env python3
import argparse
import json
import math
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("dataset")
parser.add_argument("outputs")
args = parser.parse_args()
cases = {row["id"]: row for row in json.loads(Path(args.dataset).read_text())["examples"]}
rows = [json.loads(line) for line in Path(args.outputs).read_text().splitlines() if line.strip()]
if len(rows) != len(cases) or {row["id"] for row in rows} != set(cases):
    raise SystemExit("Output IDs must match dataset IDs exactly, without duplicates.")
if not rows:
    raise SystemExit("Dataset must contain at least one case.")
for row in rows:
    if not 1 <= row["complexity"] <= 5 or not 0 <= row["confidence"] <= 1:
        raise SystemExit("Invalid difficulty or confidence in output.")
latencies = sorted(row["latency_us"] for row in rows)
def percentile(values, fraction):
    return values[max(0, math.ceil(len(values) * fraction) - 1)] if values else None
correct = [row["request_type"] == cases[row["id"]]["request_type"] for row in rows]
errors = [abs(row["complexity"] - cases[row["id"]]["complexity"]) for row in rows]
ece = 0.0
for bucket in range(10):
    selected = [i for i, row in enumerate(rows) if min(9, int(row["confidence"] * 10)) == bucket]
    if selected:
        accuracy = sum(correct[i] for i in selected) / len(selected)
        confidence = sum(rows[i]["confidence"] for i in selected) / len(selected)
        ece += len(selected) / len(rows) * abs(accuracy - confidence)
fallbacks = [row for row in rows if row.get("fallback_reason")]
costs = [row.get("decision_cost_usd") for row in rows]
report = {
    "cases": len(rows),
    "request_type_accuracy": sum(correct) / len(rows),
    "complexity_mean_absolute_error": sum(errors) / len(rows),
    "complexity_exact_accuracy": sum(error == 0 for error in errors) / len(rows),
    "ece_10_bins": ece,
    "p50_latency_us": percentile(latencies, 0.50),
    "p95_latency_us": percentile(latencies, 0.95),
    "fallback_count": len(fallbacks),
    "fallback_metadata_present": all("fallback_reason" in row for row in rows),
    "total_reported_decision_cost_usd": sum(costs) if all(isinstance(cost, (float, int)) for cost in costs) else None,
    "wrong_cases": [row["id"] for i, row in enumerate(rows) if not correct[i]],
}
print(json.dumps(report, indent=2))
