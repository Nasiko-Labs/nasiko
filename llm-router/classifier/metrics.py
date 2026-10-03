"""Score outputs locally; the official classifier_eval emits predictions only."""
import argparse
import json
from pathlib import Path

def metrics(cases, rows):
    expected = {x["id"]: x for x in cases}
    actual = {x["id"]: x for x in rows}
    if len(actual) != len(rows) or set(actual) != set(expected):
        raise ValueError("duplicate, missing or extra prediction IDs")
    paired = [(expected[k], actual[k]) for k in expected]
    n = len(paired)
    accuracy = sum(a["request_type"] == b["request_type"] for a, b in paired) / n
    mae = sum(abs(a["complexity"] - b["complexity"]) for a, b in paired) / n
    ece = 0.0
    for bucket in range(10):
        items = [(a, b) for a, b in paired if min(9, int(b["confidence"] * 10)) == bucket]
        if items:
            acc = sum(a["request_type"] == b["request_type"] for a, b in items) / len(items)
            conf = sum(b["confidence"] for a, b in items) / len(items)
            ece += len(items) / n * abs(acc - conf)
    times = sorted(b["latency_us"] for a, b in paired)
    def percentile(p):
        import math
        return times[max(0, math.ceil(p * n) - 1)] / 1000
    failures = [{"id": a["id"], "expected": a["request_type"], "predicted": b["request_type"], "confidence": b["confidence"]} for a, b in paired if a["request_type"] != b["request_type"]]
    return {"cases": n, "request_type_accuracy": accuracy, "complexity_mae": mae, "ece_10_bins": ece, "p50_ms": percentile(.5), "p95_ms": percentile(.95), "errors": failures}

def main():
    p = argparse.ArgumentParser()
    p.add_argument("--dataset", type=Path, required=True)
    p.add_argument("--outputs", type=Path, required=True)
    p.add_argument("--repeat", type=Path)
    args = p.parse_args()
    rows = [json.loads(x) for x in args.outputs.read_text(encoding="utf-8").splitlines() if x.strip()]
    result = metrics(json.loads(args.dataset.read_text(encoding="utf-8"))["examples"], rows)
    if args.repeat:
        other = [json.loads(x) for x in args.repeat.read_text(encoding="utf-8").splitlines() if x.strip()]
        result["predictions_identical"] = [{k: v for k, v in row.items() if k != "latency_us"} for row in rows] == [{k: v for k, v in row.items() if k != "latency_us"} for row in other]
    print(json.dumps(result, indent=2))

if __name__ == "__main__":
    main()
