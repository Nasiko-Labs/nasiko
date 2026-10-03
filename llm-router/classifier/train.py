"""Train and evaluate the request classifier.

Compares (all on identical grouped folds, groups = seeds so paraphrases never straddle a split):
  * embeddings (bge-small) + logistic regression   <- shipped model
  * TF-IDF + logistic regression                   <- lightweight baseline
Reports accuracy, ECE (15-bin, before/after temperature scaling) and complexity MAE, then
evaluates the shipped model on the public 10-case sample (never trained on).

    python train.py [--public /path/to/classifier-eval.json]
"""

import argparse
import json
from pathlib import Path

import joblib
import numpy as np
from scipy.optimize import minimize_scalar
from scipy.special import softmax
from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.linear_model import LogisticRegression, Ridge
from sklearn.model_selection import GroupKFold
from sklearn.pipeline import make_pipeline

from model import LABELS, featurize

HERE = Path(__file__).parent


def load(name):
    return [json.loads(line) for line in open(HERE / "data" / f"{name}.jsonl")]


def ece(probs, y, bins=15):
    conf, pred = probs.max(1), probs.argmax(1)
    edges = np.linspace(0, 1, bins + 1)
    total = 0.0
    for lo, hi in zip(edges[:-1], edges[1:]):
        m = (conf > lo) & (conf <= hi)
        if m.any():
            total += m.mean() * abs((pred[m] == y[m]).mean() - conf[m].mean())
    return total


def fit_temperature(logits, y):
    def nll(t):
        p = softmax(logits / t, axis=1)
        return -np.log(p[np.arange(len(y)), y] + 1e-12).mean()

    return float(minimize_scalar(nll, bounds=(0.05, 20), method="bounded").x)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--public", default=None)
    args = ap.parse_args()

    rows = load("train") + load("val")
    q = [r["query"] for r in rows]
    c = [r["context"] for r in rows]
    y = np.array([LABELS.index(r["request_type"]) for r in rows])
    cx = np.array([r["complexity"] for r in rows], dtype=float)
    groups = np.array([r["seed"] for r in rows])
    # Only un-augmented rows are scored in CV: augmented copies would inflate the numbers.
    is_original = np.array([i == min(j for j, g in enumerate(groups) if g == groups[i]) for i in range(len(rows))])

    X = featurize(q, c)
    text = [f"{a} {b}" for a, b in zip(q, c)]

    folds = list(GroupKFold(n_splits=5).split(X, y, groups))
    oof = {"emb": np.zeros((len(y), len(LABELS))), "tfidf": np.zeros((len(y), len(LABELS)))}
    oof_cx = np.zeros(len(y))
    for tr, te in folds:
        emb = LogisticRegression(C=4.0, max_iter=3000).fit(X[tr], y[tr])
        oof["emb"][te] = emb.decision_function(X[te])
        tf = make_pipeline(TfidfVectorizer(ngram_range=(1, 2), sublinear_tf=True), LogisticRegression(C=10, max_iter=3000))
        tf.fit([text[i] for i in tr], y[tr])
        oof["tfidf"][te] = tf.decision_function([text[i] for i in te])
        oof_cx[te] = Ridge(alpha=1.0).fit(X[tr], cx[tr]).predict(X[te])

    ev = is_original
    print(f"CV on {ev.sum()} original seeds (grouped 5-fold)")
    temps = {}
    for name, logits in oof.items():
        t = fit_temperature(logits[ev], y[ev])
        temps[name] = t
        raw, cal = softmax(logits[ev], axis=1), softmax(logits[ev] / t, axis=1)
        acc = (cal.argmax(1) == y[ev]).mean()
        print(f"  {name:6s} acc={acc:.3f}  ECE raw={ece(raw, y[ev]):.3f} calibrated={ece(cal, y[ev]):.3f}  T={t:.2f}")
    mae = np.abs(np.clip(np.rint(oof_cx[ev]), 1, 5) - cx[ev]).mean()
    within1 = (np.abs(np.clip(np.rint(oof_cx[ev]), 1, 5) - cx[ev]) <= 1).mean()
    print(f"  complexity MAE={mae:.2f}  within+-1={within1:.3f}")

    # Fallback threshold: smallest confidence where accuracy of the accepted set is >= 0.9
    cal = softmax(oof["emb"][ev] / temps["emb"], axis=1)
    conf, ok = cal.max(1), cal.argmax(1) == y[ev]
    threshold = 0.5
    for th in np.linspace(0.3, 0.95, 66):
        m = conf >= th
        if m.sum() >= 10 and ok[m].mean() >= 0.9:
            threshold = float(th)
            break
    m = conf >= threshold
    print(f"  threshold={threshold:.2f}: coverage={m.mean():.2f} accepted-acc={ok[m].mean():.3f}")

    # Final model on everything
    clf = LogisticRegression(C=4.0, max_iter=3000).fit(X, y)
    reg = Ridge(alpha=1.0).fit(X, cx)
    joblib.dump({"clf": clf, "reg": reg, "temperature": temps["emb"], "threshold": threshold, "labels": LABELS}, HERE / "model.joblib")
    print("saved model.joblib")

    if args.public:
        ex = json.load(open(args.public))["examples"]
        Xp = featurize([e["query"] for e in ex], [e.get("context", "") or "" for e in ex])
        p = softmax(clf.decision_function(Xp) / temps["emb"], axis=1)
        pred = [LABELS[i] for i in p.argmax(1)]
        correct = [a == e["request_type"] for a, e in zip(pred, ex)]
        print(f"public sample: {sum(correct)}/{len(ex)} correct, ECE={ece(p, np.array([LABELS.index(e['request_type']) for e in ex])):.3f}")
        for e, a, pr, ok_ in zip(ex, pred, p.max(1), correct):
            print(f"   {e['id']} {'ok ' if ok_ else 'BAD'} pred={a:20s} conf={pr:.2f} true={e['request_type']}")


if __name__ == "__main__":
    main()
