"""Score a classifier_eval OUT file against its EVAL_SET.

    python score.py EVAL_SET.json OUT.jsonl [--min-confidence 0.7]

Metrics (the brief's definitions):
  * accuracy     — request_type accuracy over ALL cases.
  * answered     — cases whose confidence >= --min-confidence. Below it the router would use the safe
                   default, which counts as a fallback, not an error.
  * accepted-acc — accuracy over answered cases only.
  * ECE          — expected calibration error (15 equal-width bins) over all cases.
  * complexity   — MAE and share within +-1 of the label.
  * latency      — p50/p95 of latency_us.
"""

import argparse
import json

import numpy as np


def ece(conf, correct, bins=15):
    edges = np.linspace(0, 1, bins + 1)
    total = 0.0
    for lo, hi in zip(edges[:-1], edges[1:]):
        m = (conf > lo) & (conf <= hi)
        if m.any():
            total += m.mean() * abs(correct[m].mean() - conf[m].mean())
    return total


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("eval_set")
    ap.add_argument("out")
    ap.add_argument("--min-confidence", type=float, default=0.0)
    a = ap.parse_args()

    truth = {e["id"]: e for e in json.load(open(a.eval_set))["examples"]}
    rows = [json.loads(l) for l in open(a.out) if l.strip()]
    missing = [r["id"] for r in rows if r["id"] not in truth]
    assert not missing, f"unknown ids in OUT: {missing[:3]}"

    correct = np.array([r["request_type"] == truth[r["id"]]["request_type"] for r in rows])
    conf = np.array([r["confidence"] for r in rows])
    cx_err = np.abs(np.array([r["complexity"] - truth[r["id"]]["complexity"] for r in rows]))
    lat = np.array([r["latency_us"] for r in rows])
    answered = conf >= a.min_confidence

    print(f"cases                {len(rows)}")
    print(f"accuracy (all)       {correct.mean():.3f}")
    print(f"answered @>={a.min_confidence:.2f}    {answered.mean():.3f}   accepted-acc {correct[answered].mean() if answered.any() else float('nan'):.3f}")
    print(f"ECE                  {ece(conf, correct):.3f}")
    print(f"complexity MAE       {cx_err.mean():.2f}   within+-1 {(cx_err <= 1).mean():.3f}")
    print(f"latency p50/p95      {np.percentile(lat, 50) / 1000:.2f} / {np.percentile(lat, 95) / 1000:.2f} ms")


if __name__ == "__main__":
    main()
