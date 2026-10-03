"""Train the in-process request classifier embedded at `assets/local_classifier.json`.

    python -m pip install scikit-learn numpy scipy
    python eval/train_local.py            # from llm-router/, needs cargo on PATH

Features come from Rust (`examples/local_classifier_features.rs`), so the router and this
script can never disagree on featurisation. Two multinomial logistic regressions share the
features: request type (7 classes, class-balanced) and complexity (5 levels). The
regularisation C and a softmax temperature per head are chosen by 5-fold cross-validation
on the training files only. Folds are grouped by paraphrase `family`, and each row's
robustness copies (typos, casing, padding) stay in its family, so a paraphrase or an
augmented copy can never sit on both sides of a fold. `h4-validation.json` is never read
here except for the near-duplicate check, which fails the run if a validation query leaked
into training.
"""

import json
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from scipy.sparse import csr_matrix
from sklearn.linear_model import LogisticRegression
from sklearn.model_selection import StratifiedGroupKFold

ROOT = Path(__file__).resolve().parent.parent
EVAL = ROOT / "eval"
TRAIN_FILES = sorted(EVAL.glob("train-*.jsonl"))
VALIDATION = EVAL / "h4-validation.json"
OUT = ROOT / "assets" / "local_classifier.json"
NUM_BUCKETS = 1 << 18
TYPES = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
]
TYPE_C_GRID = [8.0, 16.0, 32.0, 64.0, 128.0, 256.0]
COMPLEXITY_C_GRID = [4.0, 8.0, 16.0, 32.0, 64.0, 128.0]
T_GRID = [round(0.1 + 0.02 * i, 2) for i in range(146)]  # 0.1 .. 3.0
# Robustness copies per training row (typos / casing / padding), kept in the row's family.
AUGMENT_COPIES = 1
MAX_BUCKETS = 30_000
SEED = 7

# Boilerplate "there is no context" strings. The training files use them unevenly across
# types, which would teach the model that "No other context." means a writing request.
# Each is replaced by a seeded random pick, so they carry no label signal.
PLACEHOLDERS = [
    "No other context.",
    "No codebase context.",
    "No other files or changes needed.",
    "",
]


def load_training():
    rng = random.Random(SEED)
    rows = []
    for path in TRAIN_FILES:
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.strip():
                rows.append(json.loads(line))
    for r in rows:
        if r.get("context", "").strip() in PLACEHOLDERS:
            r["context"] = rng.choice(PLACEHOLDERS)
        assert r["request_type"] in TYPES, r["id"]
        assert 1 <= r["complexity"] <= 5, r["id"]
        r.setdefault("family", r["id"])
    return rows


PADDING = [
    ("Hey, hope you're doing well! Quick one for you. ", ""),
    ("", " Thanks so much in advance, really appreciate it!"),
    ("Sorry if this is a dumb question, I'm new here. ", " Cheers."),
    ("ok so ", " pls"),
    ("Context: I'm on a deadline today. ", " Let me know if anything is unclear."),
]


def typo(text, rng):
    chars = list(text)
    for _ in range(max(1, len(chars) // 25)):
        i = rng.randrange(len(chars) - 1) if len(chars) > 2 else 0
        op = rng.choice(["swap", "drop", "dup"])
        if op == "swap":
            chars[i], chars[i + 1] = chars[i + 1], chars[i]
        elif op == "drop":
            del chars[i]
        else:
            chars.insert(i, chars[i])
    return "".join(chars)


def augment(rows):
    """Seeded robustness copies: the same request with typos, flattened casing and
    punctuation, or a social preamble/sign-off — the surface noise real traffic has."""
    rng = random.Random(SEED + 1)
    out = []
    for r in rows:
        for k in range(AUGMENT_COPIES):
            q = r["query"]
            kind = rng.choice(["typo", "flat", "pad"])
            if kind == "typo" and len(q) > 3:
                q = typo(q, rng)
            elif kind == "flat":
                q = "".join(ch for ch in q.lower() if ch.isalnum() or ch.isspace())
            else:
                pre, post = rng.choice(PADDING)
                q = pre + q + post
            if q.strip() and q != r["query"]:
                out.append({**r, "id": f"{r['id']}~{k}", "query": q})
    return out


def shingles(text, n=5):
    t = " ".join(text.lower().split())
    return {t[i : i + n] for i in range(max(1, len(t) - n + 1))}


def check_leakage(rows):
    val = json.loads(VALIDATION.read_text(encoding="utf-8"))["examples"]
    train = [(r["id"], shingles(r["query"])) for r in rows]
    worst = []
    for v in val:
        vs = shingles(v["query"])
        best = max(((len(vs & ts) / len(vs | ts), tid) for tid, ts in train), default=(0, ""))
        worst.append((best[0], v["id"], best[1]))
    worst.sort(reverse=True)
    print("closest validation/train pairs (char 5-gram Jaccard):")
    for score, vid, tid in worst[:5]:
        print(f"  {score:.2f}  {vid} ~ {tid}")
    leaks = [w for w in worst if w[0] >= 0.7]
    if leaks:
        sys.exit(f"{len(leaks)} validation queries near-duplicate a training query; fix the data")


def dump_features(rows):
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False, encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
        tmp = f.name
    try:
        out = subprocess.run(
            ["cargo", "run", "-q", "--release", "-p", "nasiko-llm-router",
             "--example", "local_classifier_features", "--", tmp],
            cwd=ROOT, check=True, capture_output=True, text=True,
        ).stdout
    finally:
        os.unlink(tmp)
    dumped = [json.loads(l) for l in out.splitlines() if l.strip()]
    assert [d["id"] for d in dumped] == [r["id"] for r in rows]
    data, ri, ci = [], [], []
    for i, d in enumerate(dumped):
        for b, v in d["features"]:
            ri.append(i)
            ci.append(b)
            data.append(v)
    full = csr_matrix((data, (ri, ci)), shape=(len(dumped), NUM_BUCKETS))
    # Fit only on columns some row uses; weights map back to their bucket ids at the end.
    cols = np.unique(full.indices)
    return full[:, cols], cols


def nll(logits, y, t):
    z = logits / t
    z = z - z.max(axis=1, keepdims=True)
    logp = z - np.log(np.exp(z).sum(axis=1, keepdims=True))
    return -logp[np.arange(len(y)), y].mean()


def fit_head(x, y, groups, scored, c_grid, balanced, score):
    """Pick C by family-grouped 5-fold CV, then T on the out-of-fold logits at that C.
    Only original rows (`scored`) are scored — augmented copies train but never grade."""
    folds = list(
        StratifiedGroupKFold(5, shuffle=True, random_state=SEED).split(x, y, groups)
    )
    best = None
    for c in c_grid:
        oof = np.zeros((x.shape[0], len(np.unique(y))))
        for tr, te in folds:
            m = LogisticRegression(
                C=c, max_iter=3000, tol=1e-3, class_weight="balanced" if balanced else None
            ).fit(x[tr], y[tr])
            oof[te] = m.decision_function(x[te])
        s = score(oof[scored], y[scored])
        print(f"    C={c:<5} cv score {s:.4f}")
        if best is None or s > best[0]:
            best = (s, c, oof)
    _, c, oof = best
    t = min(T_GRID, key=lambda t: nll(oof[scored], y[scored], t))
    model = LogisticRegression(
        C=c, max_iter=3000, tol=1e-3, class_weight="balanced" if balanced else None
    ).fit(x, y)
    print(f"  chose C={c}, temperature={t}, cv nll {nll(oof[scored], y[scored], t):.4f}")
    return model, c, t, best[0]


def type_score(oof, y):
    return float((oof.argmax(axis=1) == y).mean())


def complexity_score(oof, y):
    z = oof - oof.max(axis=1, keepdims=True)
    p = np.exp(z) / np.exp(z).sum(axis=1, keepdims=True)
    level = p @ np.arange(5)
    return -float(np.abs(np.rint(level) - y).mean())  # higher is better: negative MAE


def main():
    originals = load_training()
    print(f"{len(originals)} training rows from {[p.name for p in TRAIN_FILES]}")
    check_leakage(originals)
    rows = originals + augment(originals)
    print(f"{len(rows) - len(originals)} robustness copies")
    scored = np.array([i < len(originals) for i in range(len(rows))])
    groups = np.array([r["family"] for r in rows])
    x, cols = dump_features(rows)
    print(f"{len(cols)} active buckets")
    y_type = np.array([TYPES.index(r["request_type"]) for r in rows])
    y_cx = np.array([r["complexity"] - 1 for r in rows])

    print("type head:")
    tm, tc, tt, tcv = fit_head(x, y_type, groups, scored, TYPE_C_GRID, True, type_score)
    print("complexity head:")
    cm, cc, ct, ccv = fit_head(
        x, y_cx, groups, scored, COMPLEXITY_C_GRID, False, complexity_score
    )
    assert list(tm.classes_) == list(range(7)) and list(cm.classes_) == list(range(5))

    w = np.hstack([tm.coef_.T, cm.coef_.T])  # (buckets, 7 + 5)
    importance = np.abs(w).max(axis=1)
    used = np.flatnonzero(importance > 0)
    keep = used[np.argsort(-importance[used])][:MAX_BUCKETS]
    keep = keep[np.argsort(cols[keep])]
    print(f"{len(used)} buckets with weight, keeping {len(keep)}")

    model = {
        "version": 1,
        "num_buckets": NUM_BUCKETS,
        "types": TYPES,
        "type_bias": [round(float(b), 5) for b in tm.intercept_],
        "complexity_bias": [round(float(b), 5) for b in cm.intercept_],
        "type_temperature": tt,
        "complexity_temperature": ct,
        "buckets": [int(cols[k]) for k in keep],
        "weights": [round(float(v), 4) for v in w[keep].ravel()],
        "provenance": {
            "trainer": "eval/train_local.py",
            "training_rows": len(originals),
            "augmented_rows": len(rows) - len(originals),
            "cv": "5-fold, stratified, grouped by paraphrase family",
            "training_files": [p.name for p in TRAIN_FILES],
            "type_head": {"C": tc, "temperature": tt, "cv_accuracy": round(tcv, 4)},
            "complexity_head": {"C": cc, "temperature": ct, "cv_mae": round(-ccv, 4)},
        },
    }
    OUT.write_text(json.dumps(model, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size / 1e6:.2f} MB)")


if __name__ == "__main__":
    main()
