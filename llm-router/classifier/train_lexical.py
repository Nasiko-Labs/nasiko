"""Train the offline lexical classifier and export it for the Rust `local` backend.

    python train_lexical.py [--public classifier-eval.json] [--out ../models/request-classifier-lexical-v1.json]

CV protocol is identical to train.py (grouped 5-fold by seed, scored on original seeds only).
"""

import argparse
import json
import math
from pathlib import Path

import numpy as np
from scipy.optimize import minimize_scalar
from scipy.sparse import csr_matrix, hstack
from scipy.special import softmax
from sklearn.linear_model import LogisticRegression, Ridge
from sklearn.model_selection import GroupKFold

import lexical
from model import LABELS
from train import ece, fit_temperature, load

HERE = Path(__file__).parent
MIN_DF = 2  # a feature must appear in at least this many distinct seeds


def build_vocab(rows):
    seed_feats = {}
    for r in rows:
        seed_feats.setdefault(r["seed"], set()).update(lexical.raw_features(r["query"], r["context"]).keys())
    df = {}
    for feats in seed_feats.values():
        for f in feats:
            df[f] = df.get(f, 0) + 1
    kept = sorted(f for f, c in df.items() if c >= MIN_DF)
    n = len(seed_feats)
    vocab = {f: i for i, f in enumerate(kept)}
    idf = [math.log((1 + n) / (1 + df[f])) + 1.0 for f in kept]
    return vocab, idf


def matrix(rows, vocab, idf):
    data, indices, indptr, dense = [], [], [0], []
    for r in rows:
        w = lexical.weigh(lexical.raw_features(r["query"], r["context"]), vocab, idf)
        indices += [i for i, _ in w]
        data += [v for _, v in w]
        indptr.append(len(indices))
        dense.append(lexical.shape(r["query"], r["context"]))
    sp = csr_matrix((data, indices, indptr), shape=(len(rows), len(vocab)))
    return hstack([sp, csr_matrix(np.array(dense))]).tocsr()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--public", default=None)
    ap.add_argument("--out", default=str(HERE.parent / "models" / "request-classifier-lexical-v1.json"))
    args = ap.parse_args()

    rows = load("train") + load("val")
    y = np.array([LABELS.index(r["request_type"]) for r in rows])
    cx = np.array([r["complexity"] for r in rows], dtype=float)
    groups = np.array([r["seed"] for r in rows])
    first = {}
    for i, g in enumerate(groups):
        first.setdefault(g, i)
    is_original = np.array([first[g] == i for i, g in enumerate(groups)])

    oof = np.zeros((len(y), len(LABELS)))
    oof_cx = np.zeros(len(y))
    for tr, te in GroupKFold(n_splits=5).split(y, y, groups):
        vocab, idf = build_vocab([rows[i] for i in tr])
        Xtr, Xte = matrix([rows[i] for i in tr], vocab, idf), matrix([rows[i] for i in te], vocab, idf)
        oof[te] = LogisticRegression(C=20.0, max_iter=5000).fit(Xtr, y[tr]).decision_function(Xte)
        oof_cx[te] = Ridge(alpha=1.0).fit(Xtr, cx[tr]).predict(Xte)

    ev = is_original
    t = fit_temperature(oof[ev], y[ev])
    cal = softmax(oof[ev] / t, axis=1)
    print(f"CV on {ev.sum()} original seeds: lexical acc={(cal.argmax(1) == y[ev]).mean():.3f} "
          f"ECE raw={ece(softmax(oof[ev], axis=1), y[ev]):.3f} calibrated={ece(cal, y[ev]):.3f} T={t:.2f}")
    err = np.abs(np.clip(np.rint(oof_cx[ev]), 1, 5) - cx[ev])
    print(f"complexity MAE={err.mean():.2f} within+-1={(err <= 1).mean():.3f}")

    # Final model on everything
    vocab, idf = build_vocab(rows)
    X = matrix(rows, vocab, idf)
    clf = LogisticRegression(C=20.0, max_iter=5000).fit(X, y)
    reg = Ridge(alpha=1.0).fit(X, cx)

    # Fallback threshold: lowest confidence whose accepted set is >= 90% accurate (>= 10 cases)
    conf, ok = cal.max(1), cal.argmax(1) == y[ev]
    threshold = 0.5
    for th in np.linspace(0.3, 0.95, 66):
        m = conf >= th
        if m.sum() >= 10 and ok[m].mean() >= 0.9:
            threshold = float(th)
            break
    m = conf >= threshold
    print(f"threshold={threshold:.2f} coverage={m.mean():.2f} accepted-acc={ok[m].mean():.3f}")

    golden_inputs = [
        ("What is the capital of France?", ""),
        ("Fix typo in this Python comment: `# retrun the cached value`.", "No other files needed."),
        ("Design a sharding scheme for a time-series store", "Writes 500k points/s, mostly last 24h; uses `fn` and {braces}."),
        ("hello there", ""),
        ("Café ÜBER straße: why is this slow?", "def f(x):  # 日本語"),
    ]
    golden = []
    for q, c in golden_inputs:
        Xg = matrix([{"query": q, "context": c, "seed": 0}], vocab, idf)
        p = softmax(clf.decision_function(Xg)[0] / t)
        golden.append({"query": q, "context": c, "probs": [round(float(v), 6) for v in p],
                       "complexity_raw": round(float(reg.predict(Xg)[0]), 6)})

    model = {
        "version": 1,
        "labels": LABELS,
        "context_scale": lexical.CONTEXT_SCALE,
        "features": list(vocab.keys()),  # index order == weight column order
        "idf": [round(v, 6) for v in idf],
        "coef": [[round(float(v), 5) for v in row] for row in clf.coef_.T.tolist()],  # [n_cols][n_labels], cols = vocab + 6 dense
        "intercept": [round(float(v), 5) for v in clf.intercept_],
        "temperature": round(t, 5),
        "threshold": round(threshold, 3),
        "cx_coef": [round(float(v), 5) for v in reg.coef_],
        "cx_intercept": round(float(reg.intercept_), 5),
        "golden": golden,
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(model, separators=(",", ":")))
    print(f"wrote {out} ({out.stat().st_size / 1024:.0f} KiB, {len(vocab)} features)")

    if args.public:
        ex = json.load(open(args.public))["examples"]
        rws = [{"query": e["query"], "context": e.get("context") or "", "seed": 0} for e in ex]
        p = softmax(clf.decision_function(matrix(rws, vocab, idf)) / t, axis=1)
        correct = [LABELS[i] == e["request_type"] for i, e in zip(p.argmax(1), ex)]
        print(f"public sample: {sum(correct)}/{len(ex)} correct")


if __name__ == "__main__":
    main()
