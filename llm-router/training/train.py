"""
P2 Training Pipeline
=====================
Trains:
  - type_clf     : 7-class request-type LogisticRegression
  - complexity_clf: 5-class complexity LogisticRegression

Uses the EXACT same feature extraction as the Rust classifier.

Steps:
  1. Load dataset.json
  2. Extract features (hashed + dense)
  3. Train both classifiers on train split
  4. Calibrate temperature on calibration split (minimize NLL)
  5. Choose threshold on validation split (maximize F1 / coverage)
  6. Export weights to llm-router/assets/request_classifier_weights.json
  7. Print evaluation report
"""

import json
import math
import sys
import time
from collections import Counter
from datetime import datetime, timezone

import numpy as np
import scipy.sparse as sp
from scipy.optimize import minimize_scalar
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score, log_loss

sys.path.insert(0, "llm-router/training")
from features import (
    NUM_BUCKETS,
    REQUEST_TYPE_CLASSES,
    COMPLEXITY_CLASSES,
    p2_hashed_features,
    p2_dense_features,
    regex_vote_features,
    classify_request_type,
)

# ─── constants ───────────────────────────────────────────────────────────────
TYPE_CLASSES    = REQUEST_TYPE_CLASSES            # ["code_generation", ...]
COMPLEXITY_VALS = list(COMPLEXITY_CLASSES)        # [1, 2, 3, 4, 5]
P2_NUM_DENSE    = 10
DENSE_TAIL      = P2_NUM_DENSE + 7               # 17


# ─── helpers ─────────────────────────────────────────────────────────────────
def softmax(logits: np.ndarray) -> np.ndarray:
    e = np.exp(logits - logits.max(axis=-1, keepdims=True))
    return e / e.sum(axis=-1, keepdims=True)


def ece(probs: np.ndarray, labels: np.ndarray, n_bins: int = 10) -> float:
    """Expected Calibration Error."""
    confidences = probs.max(axis=1)
    preds       = probs.argmax(axis=1)
    correct     = (preds == labels).astype(float)
    bins  = np.linspace(0.0, 1.0, n_bins + 1)
    total = len(labels)
    err   = 0.0
    for lo, hi in zip(bins[:-1], bins[1:]):
        mask = (confidences > lo) & (confidences <= hi)
        if mask.sum() == 0:
            continue
        acc_bin  = correct[mask].mean()
        conf_bin = confidences[mask].mean()
        err += mask.sum() / total * abs(acc_bin - conf_bin)
    return err


# ─── feature extraction (SPARSE) ─────────────────────────────────────────────
def build_sparse_feature_matrix(examples):
    """
    Returns a scipy sparse matrix of shape (N, NUM_BUCKETS + DENSE_TAIL).
    Hashed buckets → COO sparse; dense tail → dense block appended.
    This avoids allocating the massive (N, 65536) dense array.
    """
    N = len(examples)
    ncols_hashed = NUM_BUCKETS
    ncols_dense  = DENSE_TAIL

    # Collect COO data for hashed part
    rows, cols, data = [], [], []
    dense_rows = []

    for i, ex in enumerate(examples):
        q   = ex["query"]
        ctx = ex.get("context")

        hashed     = p2_hashed_features(q, ctx)
        dense      = p2_dense_features(q, ctx)
        regex_v    = regex_vote_features(q)
        dense_tail = dense + regex_v          # length 17

        for bucket, val in hashed.items():
            rows.append(i)
            cols.append(bucket)
            data.append(val)

        dense_rows.append(dense_tail)

    # Build sparse hashed block
    hashed_mat = sp.csr_matrix(
        (data, (rows, cols)),
        shape=(N, ncols_hashed),
        dtype=np.float64,
    )

    # Build dense tail block → convert to sparse
    dense_mat = np.array(dense_rows, dtype=np.float64)
    dense_sp  = sp.csr_matrix(dense_mat)

    # Horizontally stack: [hashed | dense_tail]
    X = sp.hstack([hashed_mat, dense_sp], format="csr")
    return X


# ─── encode targets ───────────────────────────────────────────────────────────
def encode_type(examples):
    return np.array([TYPE_CLASSES.index(e["request_type"]) for e in examples])

def encode_complexity(examples):
    return np.array([COMPLEXITY_VALS.index(e["complexity"]) for e in examples])


# ─── temperature calibration ─────────────────────────────────────────────────
def calibrate_temperature(raw_logits: np.ndarray, labels: np.ndarray) -> float:
    def nll(T):
        T = max(T, 0.01)
        probs = softmax(raw_logits / T)
        return log_loss(labels, probs)
    result = minimize_scalar(nll, bounds=(0.1, 5.0), method="bounded")
    return float(result.x)


# ─── threshold tuning ────────────────────────────────────────────────────────
def choose_threshold(probs: np.ndarray, labels: np.ndarray) -> float:
    preds = probs.argmax(axis=1)
    confs = probs.max(axis=1)
    total = len(labels)
    best_score = -1.0
    best_t = 0.50

    for t_int in range(40, 91, 2):
        t = t_int / 100.0
        confident     = confs >= t
        n_conf        = confident.sum()
        fallback_rate = 1.0 - n_conf / total
        if n_conf == 0 or fallback_rate > 0.40:
            continue
        acc_conf = (preds[confident] == labels[confident]).mean()
        score    = acc_conf * (1.0 - fallback_rate * 0.5)
        if score > best_score:
            best_score = score
            best_t = t

    print(f"  Threshold sweep done. best_t={best_t:.2f}, score={best_score:.4f}")
    return best_t



# ─── weight export ────────────────────────────────────────────────────────────
def export_weights(
    type_clf:        LogisticRegression,
    complexity_clf:  LogisticRegression,
    temperature:     float,
    threshold:       float,
    metadata:        dict,
) -> dict:
    """
    Converts sklearn LogisticRegression coefficients into the P2 JSON weight schema.

    Schema:
      type_hashed_weights    : {"bucket:class": weight}  sparse (non-zero only)
      type_dense_weights     : [[17 floats] × 7 classes]
      type_bias              : [7 floats]
      complexity_hashed_weights: similar
      complexity_dense_weights : [[17 floats] × 5 classes]
      complexity_bias          : [5 floats]
    """
    def coef_for(clf, n_classes, n_expected_classes):
        """Handle both binary (coef_ has 1 row) and multi (n_classes rows)."""
        coef = clf.coef_  # shape (n_classes, n_features) or (1, n_features)
        if coef.shape[0] == 1:
            # binary case – shouldn't happen but guard
            coef = np.vstack([-coef, coef])
        return coef  # (n_classes, NUM_BUCKETS + DENSE_TAIL)

    # ── Type head ──────────────────────────────────────────────────────────
    type_coef = coef_for(type_clf, 7, 7)    # (7, NUM_BUCKETS + 17)
    type_bias_list = type_clf.intercept_.tolist()

    # Hashed weights (sparse: skip near-zeros and prune noise to reduce compute & memory)
    type_hashed = {}
    threshold_sparse = 5e-4
    for cls_idx in range(7):
        for bucket in range(NUM_BUCKETS):
            w = float(type_coef[cls_idx, bucket])
            if abs(w) > threshold_sparse:
                type_hashed[f"{bucket}:{cls_idx}"] = w

    # Dense + regex tail weights
    type_dense = type_coef[:, NUM_BUCKETS:].tolist()   # [[17] × 7]

    # ── Complexity head ────────────────────────────────────────────────────
    comp_coef = coef_for(complexity_clf, 5, 5)   # (5, NUM_BUCKETS + 17)
    comp_bias_list = complexity_clf.intercept_.tolist()

    comp_hashed = {}
    for cls_idx in range(5):
        for bucket in range(NUM_BUCKETS):
            w = float(comp_coef[cls_idx, bucket])
            if abs(w) > threshold_sparse:
                comp_hashed[f"{bucket}:{cls_idx}"] = w

    comp_dense = comp_coef[:, NUM_BUCKETS:].tolist()   # [[17] × 5]

    payload = {
        "schema":                   "p2-v1",
        "num_buckets":              NUM_BUCKETS,
        "num_dense_features":       P2_NUM_DENSE,
        "temperature":              round(temperature, 6),
        "type_bias":                [round(b, 8) for b in type_bias_list],
        "complexity_bias":          [round(b, 8) for b in comp_bias_list],
        "type_hashed_weights":      {k: round(v, 8) for k, v in type_hashed.items()},
        "type_dense_weights":       [[round(w, 8) for w in row] for row in type_dense],
        "complexity_hashed_weights":{k: round(v, 8) for k, v in comp_hashed.items()},
        "complexity_dense_weights": [[round(w, 8) for w in row] for row in comp_dense],
        "trained_at":               metadata["trained_at"],
        "metadata":                 metadata,
    }
    return payload


# ─── main ────────────────────────────────────────────────────────────────────
def main():
    print("=" * 60)
    print("P2 Training Pipeline")
    print("=" * 60)

    # ── Load dataset ──────────────────────────────────────────────────────
    with open("llm-router/training/dataset.json", encoding="utf-8") as f:
        ds = json.load(f)["dataset"]

    train_ex  = [d for d in ds if d["split"] == "train"]
    calib_ex  = [d for d in ds if d["split"] == "calibration"]
    val_ex    = [d for d in ds if d["split"] == "validation"]

    print(f"Dataset: train={len(train_ex)}, calib={len(calib_ex)}, val={len(val_ex)}")
    print(f"Train class dist: {Counter(e['request_type'] for e in train_ex)}")
    print()

    # ── Extract features (sparse) ─────────────────────────────────────────
    print("Extracting sparse feature matrices…")
    t0 = time.perf_counter()
    X_tr = build_sparse_feature_matrix(train_ex)
    X_ca = build_sparse_feature_matrix(calib_ex)
    X_va = build_sparse_feature_matrix(val_ex)
    print(f"  Done in {time.perf_counter()-t0:.1f}s | shape train={X_tr.shape}, nnz={X_tr.nnz}")

    y_type_tr   = encode_type(train_ex)
    y_type_ca   = encode_type(calib_ex)
    y_type_va   = encode_type(val_ex)
    y_comp_tr   = encode_complexity(train_ex)
    y_comp_ca   = encode_complexity(calib_ex)
    y_comp_va   = encode_complexity(val_ex)

    # ── Train classifiers ─────────────────────────────────────────────────
    # lbfgs handles sparse matrices well and converges faster than saga on small N
    print("\nTraining type_clf (7-class LogisticRegression, lbfgs)…")
    t2 = time.perf_counter()
    type_clf = LogisticRegression(
        C=1.0,
        solver="lbfgs",
        max_iter=1000,
        class_weight="balanced",
        random_state=42,
        tol=1e-4,
    )
    type_clf.fit(X_tr, y_type_tr)
    print(f"  type_clf trained in {time.perf_counter()-t2:.1f}s")

    print("Training complexity_clf (5-class LogisticRegression, lbfgs)...")
    t3 = time.perf_counter()
    from collections import Counter as Cnt
    comp_counts = Cnt(y_comp_tr.tolist())
    n_comp_total = len(y_comp_tr)
    # Balanced weights baseline
    comp_cw = {i: n_comp_total / (5.0 * comp_counts[i]) for i in range(5)}
    # Boost Level 3 (class index 2) by 2.5x to counter L2 prior and improve exact accuracy
    comp_cw[2] *= 2.5
    complexity_clf = LogisticRegression(
        C=2.0,
        solver="lbfgs",
        max_iter=1000,
        class_weight=comp_cw,
        random_state=42,
        tol=1e-4,
    )
    complexity_clf.fit(X_tr, y_comp_tr)
    print(f"  complexity_clf trained in {time.perf_counter()-t3:.1f}s")

    # ── Calibrate temperature on calibration split ────────────────────────
    print("\nCalibrating temperature on calibration split…")
    raw_logits_ca = type_clf.decision_function(X_ca)   # (N_ca, 7)
    temperature   = calibrate_temperature(raw_logits_ca, y_type_ca)
    print(f"  Calibrated temperature = {temperature:.4f}")

    # ── Choose threshold on validation split ──────────────────────────────
    print("\nChoosing confidence threshold on validation split…")
    raw_logits_va    = type_clf.decision_function(X_va)
    type_probs_va    = softmax(raw_logits_va / temperature)
    threshold = choose_threshold(type_probs_va, y_type_va)
    print(f"  Chosen threshold = {threshold:.2f}")

    # -- Compute metrics ---------------------------------------------------
    print("\n-- Validation metrics --------------------------------------")

    # Type accuracy
    type_preds_va   = type_probs_va.argmax(axis=1)
    type_acc        = accuracy_score(y_type_va, type_preds_va)

    # Complexity accuracy
    comp_probs_va   = softmax(complexity_clf.decision_function(X_va))
    comp_preds_va   = comp_probs_va.argmax(axis=1)
    comp_acc        = accuracy_score(y_comp_va, comp_preds_va)

    # ECE
    ece_val         = ece(type_probs_va, y_type_va)

    # Fallback rate at chosen threshold
    confs_va        = type_probs_va.max(axis=1)
    fallback_rate   = (confs_va < threshold).mean()

    # Compare vs regex baseline
    regex_preds = np.array([
        TYPE_CLASSES.index(classify_request_type(e["query"])) for e in val_ex
    ])
    regex_acc   = accuracy_score(y_type_va, regex_preds)

    regex_wrong_ml_correct = int(
        ((regex_preds != y_type_va) & (type_preds_va == y_type_va)).sum()
    )
    regex_correct_ml_wrong = int(
        ((regex_preds == y_type_va) & (type_preds_va != y_type_va)).sum()
    )

    print(f"  Type accuracy  (ML):    {type_acc:.4f}")
    print(f"  Type accuracy  (Regex): {regex_acc:.4f}")
    print(f"  Complexity accuracy:    {comp_acc:.4f}")
    print(f"  ECE:                    {ece_val:.4f}")
    print(f"  Fallback rate:          {fallback_rate:.4f}")
    print(f"  Regex wrong -> ML correct: {regex_wrong_ml_correct}")
    print(f"  Regex correct -> ML wrong: {regex_correct_ml_wrong}")

    # Training metrics
    type_acc_tr = accuracy_score(y_type_tr, type_clf.predict(X_tr))
    comp_acc_tr = accuracy_score(y_comp_tr, complexity_clf.predict(X_tr))
    print(f"\n  Train type accuracy:       {type_acc_tr:.4f}")
    print(f"  Train complexity accuracy: {comp_acc_tr:.4f}")

    # Per-class accuracy on validation
    print("\n  Per-class accuracy on validation split:")
    for i, cls in enumerate(TYPE_CLASSES):
        mask     = y_type_va == i
        n        = mask.sum()
        if n == 0:
            print(f"    {cls:<25}: N/A")
        else:
            cls_acc  = (type_preds_va[mask] == i).mean()
            print(f"    {cls:<25}: {cls_acc:.3f} (N={n})")

    # -- Export weights ----------------------------------------------------
    print("\nExporting weights...")
    metadata = {
        "trained_at": datetime.now(timezone.utc).isoformat(),
        "type_accuracy_val": round(type_acc, 4),
        "complexity_accuracy_val": round(comp_acc, 4),
        "ece_val": round(ece_val, 4),
        "fallback_rate_val": round(fallback_rate, 4),
        "temperature": round(temperature, 6),
        "threshold": round(threshold, 2),
        "n_train": len(train_ex),
        "n_calib": len(calib_ex),
        "n_val": len(val_ex),
        "type_solver": "saga",
        "type_C": 0.5,
        "complexity_C": 0.5,
        "num_buckets": NUM_BUCKETS,
        "num_dense": P2_NUM_DENSE,
        "regex_wrong_ml_correct": regex_wrong_ml_correct,
        "regex_correct_ml_wrong": regex_correct_ml_wrong,
    }

    payload = export_weights(
        type_clf, complexity_clf, temperature, threshold, metadata
    )

    out_path = "llm-router/assets/request_classifier_weights.json"
    with open(out_path, "w", encoding="utf-8") as f:
        json.dump(payload, f, indent=2)

    n_type_sparse  = len(payload["type_hashed_weights"])
    n_comp_sparse  = len(payload["complexity_hashed_weights"])
    import os
    file_size_kb = os.path.getsize(out_path) / 1024

    print(f"  Saved to {out_path}")
    print(f"  type_hashed entries:   {n_type_sparse:,}")
    print(f"  comp_hashed entries:   {n_comp_sparse:,}")
    print(f"  File size:             {file_size_kb:.1f} KB")
    print(f"  Temperature:           {temperature:.4f}")
    print(f"  Threshold:             {threshold:.2f}")

    # -- Final verdict -----------------------------------------------------
    print("\n-- Final decision -------------------------------------------")
    if type_acc > regex_acc and ece_val < 0.15 and fallback_rate < 0.35:
        verdict = "KEEP ML"
    elif type_acc > regex_acc:
        verdict = "KEEP ML + FALLBACK"
    else:
        verdict = "REVERT TO REGEX BASELINE"
    print(f"  {verdict}")
    print("\nDone.")


if __name__ == "__main__":
    main()
