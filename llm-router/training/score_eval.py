"""
Score regex-out.jsonl vs ml-out.jsonl against eval_val_set.json ground truth.
"""
import json
import sys
from collections import defaultdict

def load_jsonl(path):
    rows = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                obj = json.loads(line)
                rows[obj["id"]] = obj
    return rows

def load_gt(path):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    return {e["id"]: e for e in data["examples"]}

def score(pred_rows, gt):
    ids = list(gt.keys())
    n = len(ids)
    type_correct = 0
    comp_correct = 0
    latencies = []
    fallbacks = 0
    per_class = defaultdict(lambda: {"correct": 0, "total": 0})

    for id_ in ids:
        p = pred_rows.get(id_, {})
        g = gt[id_]
        gt_type = g["ground_truth_type"]
        gt_comp = g["ground_truth_complexity"]

        pred_type = p.get("request_type", "general")
        pred_comp = p.get("complexity")
        confidence = p.get("confidence")
        lat = p.get("latency_us", 0)

        latencies.append(lat)

        if pred_type == gt_type:
            type_correct += 1
        per_class[gt_type]["total"] += 1
        if pred_type == gt_type:
            per_class[gt_type]["correct"] += 1

        if pred_comp is not None and pred_comp == gt_comp:
            comp_correct += 1

        if confidence is None:
            fallbacks += 1

    latencies.sort()
    p50 = latencies[len(latencies) // 2] if latencies else 0
    p95 = latencies[int(len(latencies) * 0.95)] if latencies else 0

    return {
        "n": n,
        "type_acc": type_correct / n,
        "comp_acc": comp_correct / n,
        "fallback_rate": fallbacks / n,
        "latency_p50_us": p50,
        "latency_p95_us": p95,
        "per_class": {k: v["correct"] / v["total"] for k, v in per_class.items()},
    }

def compare(regex_rows, ml_rows, gt):
    ids = list(gt.keys())
    gt_labels = {id_: gt[id_]["ground_truth_type"] for id_ in ids}

    regex_correct_set = {id_ for id_ in ids if regex_rows.get(id_, {}).get("request_type") == gt_labels[id_]}
    ml_correct_set    = {id_ for id_ in ids if ml_rows.get(id_, {}).get("request_type") == gt_labels[id_]}

    regex_wrong_ml_right = ml_correct_set - regex_correct_set
    regex_right_ml_wrong = regex_correct_set - ml_correct_set
    both_wrong           = (set(ids) - regex_correct_set) - ml_correct_set

    return {
        "regex_wrong_ml_correct": len(regex_wrong_ml_right),
        "regex_correct_ml_wrong": len(regex_right_ml_wrong),
        "both_wrong": len(both_wrong),
        "both_correct": len(regex_correct_set & ml_correct_set),
    }

def main():
    gt         = load_gt("llm-router/training/eval_val_set.json")
    regex_rows = load_jsonl("regex-out.jsonl")
    ml_rows    = load_jsonl("ml-out.jsonl")

    rs = score(regex_rows, gt)
    ms = score(ml_rows, gt)
    cmp = compare(regex_rows, ml_rows, gt)

    print("=" * 64)
    print("P2 Classifier Evaluation Report")
    print("=" * 64)
    print(f"Eval set size: {rs['n']} examples\n")

    print(f"{'Metric':<30} {'Regex':>10} {'ML':>10}")
    print("-" * 52)
    print(f"{'Type accuracy':<30} {rs['type_acc']:>10.4f} {ms['type_acc']:>10.4f}")
    print(f"{'Complexity accuracy':<30} {rs['comp_acc']:>10.4f} {ms['comp_acc']:>10.4f}")
    print(f"{'Fallback rate':<30} {rs['fallback_rate']:>10.4f} {ms['fallback_rate']:>10.4f}")
    print(f"{'Latency p50 (µs)':<30} {rs['latency_p50_us']:>10} {ms['latency_p50_us']:>10}")
    print(f"{'Latency p95 (µs)':<30} {rs['latency_p95_us']:>10} {ms['latency_p95_us']:>10}")

    print(f"\n{'Confusion matrix'}")
    print(f"  Regex wrong → ML correct:  {cmp['regex_wrong_ml_correct']}")
    print(f"  Regex correct → ML wrong:  {cmp['regex_correct_ml_wrong']}")
    print(f"  Both correct:              {cmp['both_correct']}")
    print(f"  Both wrong:                {cmp['both_wrong']}")

    print(f"\n{'Per-class accuracy (ML vs Regex)':}")
    all_classes = sorted(set(list(rs['per_class'].keys()) + list(ms['per_class'].keys())))
    print(f"  {'Class':<28} {'Regex':>7} {'ML':>7}")
    print(f"  {'-'*44}")
    for cls in all_classes:
        r = rs['per_class'].get(cls, 0.0)
        m = ms['per_class'].get(cls, 0.0)
        delta = "↑" if m > r else ("↓" if m < r else "=")
        print(f"  {cls:<28} {r:>7.3f} {m:>7.3f}  {delta}")

    delta_acc = ms['type_acc'] - rs['type_acc']
    print(f"\n{'─'*52}")
    print(f"  ML improvement: {delta_acc:+.4f} ({delta_acc*100:+.1f}pp)")
    if ms['type_acc'] > rs['type_acc'] and ms['fallback_rate'] < 0.35:
        verdict = "✅  KEEP ML — better accuracy, acceptable fallback rate"
    elif ms['type_acc'] > rs['type_acc']:
        verdict = "✅  KEEP ML + FALLBACK — better accuracy, higher fallback"
    else:
        verdict = "⚠️  REVERT — ML does not improve over regex baseline"
    print(f"  Verdict: {verdict}")
    print("=" * 64)

if __name__ == "__main__":
    main()
