"""Score ``classifier_eval`` output against a labelled dataset.

    python score.py --gold ../data/val.json --pred /tmp/out.jsonl [--min-confidence 0.5]

Reports request-type accuracy, expected calibration error, accuracy per slice tag,
complexity exact/MAE, and latency percentiles. With ``--min-confidence``, answers below the
threshold are counted as fallbacks (routed to the safe default) rather than errors, the way
the brief scores them, and accuracy on the answered remainder is reported too.
"""

import argparse
import json
from collections import defaultdict

import numpy as np

ECE_BINS = 10


def ece(conf, correct):
    total = 0.0
    for lo in np.linspace(0, 1, ECE_BINS, endpoint=False):
        mask = (conf > lo) & (conf <= lo + 1 / ECE_BINS)
        if mask.any():
            total += mask.mean() * abs(correct[mask].mean() - conf[mask].mean())
    return float(total)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--gold", required=True)
    ap.add_argument("--pred", required=True)
    ap.add_argument("--min-confidence", type=float, default=None)
    args = ap.parse_args()

    gold = {e["id"]: e for e in json.load(open(args.gold, encoding="utf-8"))["examples"]}
    preds = [json.loads(line) for line in open(args.pred, encoding="utf-8") if line.strip()]
    assert {p["id"] for p in preds} == set(gold), "prediction ids do not match the gold set"

    correct = np.array([p["request_type"] == gold[p["id"]]["request_type"] for p in preds])
    conf = np.array([p["confidence"] for p in preds], dtype=float)
    cx_pred = np.array([p["complexity"] for p in preds])
    cx_gold = np.array([gold[p["id"]]["complexity"] for p in preds])
    latency = np.array([p["latency_us"] for p in preds])

    report = {
        "n": len(preds),
        "type_accuracy": round(float(correct.mean()), 4),
        "type_ece": round(ece(conf, correct), 4),
        "complexity_exact": round(float((cx_pred == cx_gold).mean()), 4),
        "complexity_mae": round(float(np.abs(cx_pred - cx_gold).mean()), 4),
        "latency_p50_us": int(np.percentile(latency, 50)),
        "latency_p95_us": int(np.percentile(latency, 95)),
    }
    if args.min_confidence is not None:
        answered = conf >= args.min_confidence
        report["fallback_rate"] = round(float((~answered).mean()), 4)
        report["accuracy_when_answered"] = round(float(correct[answered].mean()), 4) if answered.any() else None
        report["confident_errors"] = int((answered & ~correct).sum())

    by_tag = defaultdict(list)
    by_label = defaultdict(list)
    for p, ok in zip(preds, correct):
        for tag in gold[p["id"]].get("tests", []):
            by_tag[tag].append(ok)
        by_label[gold[p["id"]]["request_type"]].append(ok)
    report["accuracy_by_slice"] = {t: round(float(np.mean(v)), 3) for t, v in sorted(by_tag.items())}
    report["recall_by_label"] = {t: round(float(np.mean(v)), 3) for t, v in sorted(by_label.items())}
    print(json.dumps(report, indent=1))


if __name__ == "__main__":
    main()
