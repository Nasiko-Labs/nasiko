#!/usr/bin/env python3
"""Train and export the local request-classifier model.

Reads `training/dataset.jsonl`, featurizes each example with a byte-for-byte reimplementation
of the Rust feature engine in `llm-router/src/routing/features.rs` (FNV-1a hashing trick over
word 1-2grams + char 3-5grams, plus six dense features), trains:

  * a multinomial logistic regression over the seven `RequestType` labels, and
  * a ridge regressor for complexity 1-5,

fits a single temperature on the validation split for confidence calibration, and writes the
weights to `llm-router/assets/request_classifier_weights.json` in the schema the Rust loader
(`routing/request_classifier.rs`) consumes, plus held-out metrics to `training/metrics.json`.

Determinism: no sampling, fixed seeds; `trained_at_utc` is pinned so re-running with the same
scikit-learn/numpy versions produces the same bytes. Run:

    python3 training/train_request_classifier.py

Requires numpy + scikit-learn (declared in training/README.md). Training is not part of the
Rust build; the exported JSON is committed and embedded, so `cargo build` needs no Python.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
from pathlib import Path

import numpy as np
from scipy import sparse
from sklearn.linear_model import LogisticRegression, Ridge
from sklearn.model_selection import StratifiedKFold, cross_val_score

HERE = Path(__file__).resolve().parent
REPO_LLM_ROUTER = HERE.parent
ASSET_PATH = REPO_LLM_ROUTER / "assets" / "request_classifier_weights.json"
METRICS_PATH = HERE / "metrics.json"

# Canonical label order — MUST match `RequestType`'s `as_str` order in
# `llm-router/src/routing/classifier.rs` (the Rust loader validates this list).
LABELS = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
]

# Feature-engine constants — MUST match `routing/request_classifier.rs`.
NUM_BUCKETS = 1 << 12  # 4096 — selected on train-only CV; the smaller space regularizes the
                       # hashed n-grams and keeps the embedded asset compact.
NUM_DENSE = 6
WORD_MAX_N = 2
CHAR_MIN_N = 3
CHAR_MAX_N = 5
# Context contributes its own n-grams at this weight, normalized separately from the query.
# Must match the Rust engine's `CONTEXT_WEIGHT`.
CONTEXT_WEIGHT = 0.5

# Constraint cue words for dense feature 5 (complexity signal).
CONSTRAINT_CUES = [
    "must", "should", "do not", "don't", "without", "only", "at least",
    "ensure", "exactly", "before", "after", "instead", "reject", "never",
]

# Pinned provenance timestamp so regeneration is byte-stable (override with SOURCE_DATE_EPOCH).
TRAINED_AT = "2026-10-03T00:00:00Z"


# ---------------------------------------------------------------------------
# Feature engine — mirrors src/routing/features.rs and request_classifier.rs
# ---------------------------------------------------------------------------

def fnv1a(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for byte in data:
        h ^= byte
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def hash_to_bucket(gram: str, num_buckets: int) -> tuple[int, float]:
    bucket = fnv1a(gram.encode("utf-8")) % num_buckets
    sign_bit = fnv1a(("sign:" + gram).encode("utf-8")) & 1
    return bucket, (1.0 if sign_bit == 0 else -1.0)


def word_tokens(text: str) -> list[str]:
    tokens, cur = [], []
    for ch in text.lower():
        if ch.isalnum():
            cur.append(ch)
        else:
            if cur:
                tokens.append("".join(cur))
                cur = []
    if cur:
        tokens.append("".join(cur))
    return tokens


def word_ngrams(tokens: list[str], max_n: int) -> list[str]:
    grams: list[str] = []
    for n in range(1, max_n + 1):
        if n > len(tokens):
            break
        for i in range(len(tokens) - n + 1):
            grams.append(" ".join(tokens[i:i + n]))
    return grams


def char_ngrams(text: str, min_n: int, max_n: int) -> list[str]:
    chars = list(text.lower())
    grams: list[str] = []
    for n in range(min_n, max_n + 1):
        if n > len(chars):
            break
        for i in range(len(chars) - n + 1):
            grams.append("".join(chars[i:i + n]))
    return grams


def combined_text(query: str, context: str) -> str:
    context = (context or "").strip()
    return query if not context else f"{query}\n{context}"


def hashed_feature_sum(text: str) -> dict[int, float]:
    tokens = word_tokens(text)
    grams = word_ngrams(tokens, WORD_MAX_N)
    grams.extend(char_ngrams(text, CHAR_MIN_N, CHAR_MAX_N))
    norm = max(math.sqrt(len(grams)), 1.0)
    out: dict[int, float] = {}
    for gram in grams:
        bucket, sign = hash_to_bucket(gram, NUM_BUCKETS)
        out[bucket] = out.get(bucket, 0.0) + sign / norm
    return out


def hashed_features(query: str, context: str) -> dict[int, float]:
    """Query features, plus context features at a reduced weight.

    Query and context are featurized and *normalized separately* — a long context must not
    dilute the query's feature magnitudes — and the context contributes at
    `CONTEXT_WEIGHT` so the request type stays driven mainly by what the user asked, with
    context as a secondary signal. The Rust engine mirrors this exactly.
    """
    out = hashed_feature_sum(query)
    ctx = (context or "").strip()
    if ctx:
        for bucket, value in hashed_feature_sum(ctx).items():
            out[bucket] = out.get(bucket, 0.0) + CONTEXT_WEIGHT * value
    return out


def dense_features(query: str, context: str) -> list[float]:
    text = combined_text(query, context)
    # Normalize the context the same way `combined_text` does, so a whitespace-only context
    # is identical to no context (feature 2) — the Rust engine mirrors this exactly.
    context = (context or "").strip()
    lower = text.lower()
    constraint_hits = sum(1 for cue in CONSTRAINT_CUES if cue in lower)
    return [
        math.log1p(len(word_tokens(query))),
        math.log1p(len(query)),
        math.log1p(len(context)),
        1.0 if "`" in text else 0.0,
        1.0 if "?" in query else 0.0,
        math.log1p(constraint_hits),
    ]


def featurize(rows: list[dict]) -> sparse.csr_matrix:
    n_features = NUM_BUCKETS + NUM_DENSE
    indptr = [0]
    indices: list[int] = []
    data: list[float] = []
    for row in rows:
        feats = hashed_features(row["query"], row.get("context", ""))
        for bucket, value in feats.items():
            indices.append(bucket)
            data.append(value)
        for i, value in enumerate(dense_features(row["query"], row.get("context", ""))):
            if value != 0.0:
                indices.append(NUM_BUCKETS + i)
                data.append(value)
        indptr.append(len(indices))
    return sparse.csr_matrix(
        (np.asarray(data, dtype=np.float64), np.asarray(indices, dtype=np.int64),
         np.asarray(indptr, dtype=np.int64)),
        shape=(len(rows), n_features),
    )


# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------

def softmax(logits: np.ndarray) -> np.ndarray:
    z = logits - logits.max(axis=1, keepdims=True)
    e = np.exp(z)
    return e / e.sum(axis=1, keepdims=True)


def expected_calibration_error(probs: np.ndarray, correct: np.ndarray, bins: int = 10) -> float:
    conf = probs.max(axis=1)
    pred = probs.argmax(axis=1)
    acc = (pred == correct).astype(np.float64)
    ece = 0.0
    edges = np.linspace(0.0, 1.0, bins + 1)
    for lo, hi in zip(edges[:-1], edges[1:]):
        mask = (conf > lo) & (conf <= hi)
        if mask.sum() == 0:
            continue
        ece += mask.mean() * abs(acc[mask].mean() - conf[mask].mean())
    return float(ece)


def classification_report(y: np.ndarray, probs: np.ndarray) -> dict:
    pred = probs.argmax(axis=1)
    per_class = {}
    recalls = []
    for i, label in enumerate(LABELS):
        support = int((y == i).sum())
        hits = int(((y == i) & (pred == i)).sum())
        recall = hits / support if support else 0.0
        recalls.append(recall)
        per_class[label] = {"support": support, "recall": round(recall, 4)}
    return {
        "accuracy": round(float((pred == y).mean()), 4),
        "macro_recall": round(float(np.mean(recalls)), 4),
        "per_class_recall": per_class,
        "ece_top1": round(expected_calibration_error(probs, y), 4),
    }


def fit_temperature(val_logits: np.ndarray, y: np.ndarray) -> float:
    best_t, best_nll = 1.0, float("inf")
    for t in np.linspace(0.5, 5.0, 91):
        probs = softmax(val_logits / t)
        nll = -np.log(np.clip(probs[np.arange(len(y)), y], 1e-12, None)).mean()
        if nll < best_nll:
            best_nll, best_t = nll, float(t)
    return round(best_t, 4)


# ---------------------------------------------------------------------------
# Training
# ---------------------------------------------------------------------------

def load_rows() -> list[dict]:
    rows = [json.loads(line) for line in (HERE / "dataset.jsonl").read_text().splitlines() if line]
    assert all(r["request_type"] in LABELS for r in rows)
    return rows


def select_model(x_tr, y_tr):
    """Grid search via 5-fold CV on the *train* split only.

    Selecting on the (small) val split would overfit it — and the val split is what fits the
    temperature, so it must stay clean for that. CV accuracy picks the winner; mean log loss
    breaks ties. The test split is never consulted here.
    """
    cv = StratifiedKFold(n_splits=5, shuffle=True, random_state=0)
    best = None
    for c in [0.1, 0.3, 1.0, 3.0, 10.0]:
        for balanced in [False, True]:
            clf = LogisticRegression(
                C=c, max_iter=3000, solver="lbfgs",
                class_weight="balanced" if balanced else None, random_state=0,
            )
            acc = float(cross_val_score(clf, x_tr, y_tr, cv=cv, scoring="accuracy").mean())
            nll = float(-cross_val_score(
                clf, x_tr, y_tr, cv=cv, scoring="neg_log_loss").mean())
            key = (round(acc, 4), round(nll, 4))
            if best is None or key > best[0]:
                best = (key, clf, {"C": c, "class_weight_balanced": balanced,
                                   "cv_accuracy": round(acc, 4), "cv_log_loss": round(nll, 4)})
    clf = best[1].fit(x_tr, y_tr)
    return clf, best[2]


def export_weights(clf: LogisticRegression, ridge: Ridge, temperature: float,
                   rows: list[dict]) -> dict:
    coef = clf.coef_  # (n_classes, n_features)
    intercept = clf.intercept_
    hashed_w = coef[:, :NUM_BUCKETS]
    dense_w = coef[:, NUM_BUCKETS:]

    # Sparse export: only buckets any class gives non-trivial weight to.
    bucket_vectors: dict[str, list[float]] = {}
    nz_buckets = np.unique(np.nonzero(np.abs(hashed_w) > 1e-9)[1])
    for b in nz_buckets:
        bucket_vectors[str(int(b))] = [round(float(v), 8) for v in hashed_w[:, b]]

    c_hashed = ridge.coef_[:NUM_BUCKETS]
    c_bucket: dict[str, float] = {}
    for b in np.unique(np.nonzero(np.abs(c_hashed) > 1e-9)[0]):
        c_bucket[str(int(b))] = round(float(c_hashed[b]), 8)

    dataset = (HERE / "dataset.jsonl").read_bytes()
    return {
        "schema": "nasiko-request-classifier-weights-v1",
        "num_buckets": NUM_BUCKETS,
        "num_dense_features": NUM_DENSE,
        "word_max_n": WORD_MAX_N,
        "char_min_n": CHAR_MIN_N,
        "char_max_n": CHAR_MAX_N,
        "context_weight": CONTEXT_WEIGHT,
        "labels": LABELS,
        "bias": [round(float(v), 8) for v in intercept],
        "dense_weights": [[round(float(v), 8) for v in row] for row in dense_w],
        "hashed_weights": bucket_vectors,
        "temperature": temperature,
        "complexity": {
            "bias": round(float(ridge.intercept_), 8),
            "dense_weights": [round(float(v), 8) for v in ridge.coef_[NUM_BUCKETS:]],
            "hashed_weights": c_bucket,
        },
        "provenance": {
            "trained_at_utc": os.environ.get("SOURCE_DATE_EPOCH_ISO", TRAINED_AT),
            "dataset_sha256": hashlib.sha256(dataset).hexdigest(),
            "train_examples": sum(1 for r in rows if r["split"] == "train"),
            "val_examples": sum(1 for r in rows if r["split"] == "val"),
            "test_examples": sum(1 for r in rows if r["split"] == "test"),
            "feature_engine": "fnv1a hashing + word 1-2gram + char 3-5gram + 6 dense",
            "training_script": "llm-router/training/train_request_classifier.py",
        },
    }


def main() -> None:
    rows = load_rows()
    train = [r for r in rows if r["split"] == "train"]
    val = [r for r in rows if r["split"] == "val"]
    test = [r for r in rows if r["split"] == "test"]

    x_tr, x_val, x_test = featurize(train), featurize(val), featurize(test)
    y_tr = np.array([LABELS.index(r["request_type"]) for r in train])
    y_val = np.array([LABELS.index(r["request_type"]) for r in val])
    y_test = np.array([LABELS.index(r["request_type"]) for r in test])

    clf, clf_choice = select_model(x_tr, y_tr)
    ridge = Ridge(alpha=1.0, random_state=0)
    ridge.fit(x_tr, np.array([r["complexity"] for r in train], dtype=np.float64))

    temperature = fit_temperature(clf.decision_function(x_val), y_val)

    metrics = {
        "model_choice": clf_choice,
        "temperature": temperature,
        "splits": {},
        "complexity": {},
        "baseline_note": "Regex baseline is measured by the Rust example "
                         "(CLASSIFIER_BACKEND=regex) on the same eval_sets, not reimplemented here.",
    }
    for name, x, y in [("train", x_tr, y_tr), ("val", x_val, y_val), ("test", x_test, y_test)]:
        probs = softmax(clf.decision_function(x) / temperature)
        metrics["splits"][name] = classification_report(y, probs)

    for name, split_rows, x in [("train", train, x_tr), ("val", val, x_val), ("test", test, x_test)]:
        y_true = np.array([r["complexity"] for r in split_rows], dtype=np.float64)
        pred = np.clip(np.rint(ridge.predict(x)), 1, 5)
        metrics["complexity"][name] = {
            "mae": round(float(np.abs(pred - y_true).mean()), 4),
            "exact_match": round(float((pred == y_true).mean()), 4),
            "within_one": round(float((np.abs(pred - y_true) <= 1).mean()), 4),
        }

    weights = export_weights(clf, ridge, temperature, rows)
    ASSET_PATH.write_text(json.dumps(weights, indent=1) + "\n", encoding="utf-8")
    METRICS_PATH.write_text(json.dumps(metrics, indent=2) + "\n", encoding="utf-8")

    print(f"wrote {ASSET_PATH} ({ASSET_PATH.stat().st_size / 1024:.0f} KiB, "
          f"{len(weights['hashed_weights'])} hashed buckets)")
    print(f"chosen: {clf_choice}")
    print(f"temperature: {temperature}")
    for split in ("train", "val", "test"):
        m = metrics["splits"][split]
        c = metrics["complexity"][split]
        print(f"  {split:5s} acc={m['accuracy']:.3f} macro_recall={m['macro_recall']:.3f} "
              f"ece={m['ece_top1']:.3f} | complexity mae={c['mae']:.2f} ±1={c['within_one']:.2f}")


if __name__ == "__main__":
    main()
