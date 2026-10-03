#!/usr/bin/env python3
"""Request-classifier data + training pipeline (offline tooling; not part of the cargo build).

Subcommands (run from the repo root; every step is deterministic, seed 13):

  build   data/base/*.jsonl  -> data/raw.jsonl        assign ids/groups, auto-tags, label-preserving variants
  split   data/raw.jsonl     -> data/labelled.jsonl   group split 70/15/15 + near-dup guards
                             -> data/eval_{val,test}.json  (public eval schema, for classifier_eval)
  (dump)  cargo run --release -p nasiko-llm-router --example dump_classifier_features   (Rust computes features)
  train   out/features.jsonl -> out/model.npz         type head + Frank-Hall complexity head + temperature
                                                      (C and T chosen by group 5-fold CV on train only)
  eval    out/model.npz      -> out/val_report.md     validation metrics (regex vs local raw vs local+T)
  export  out/model.npz      -> llm-router/assets/request_classifier.json
  report  --eval FILE --out OUT.jsonl [--out-b OUT2.jsonl]   metrics from the harness's own output
  kappa   --a FILE --b FILE  agreement between two label sets on shared ids

Python never computes a feature: it fits on vectors the Rust feature engine wrote, so training and
serving cannot drift apart.
"""
from __future__ import annotations

import argparse
import datetime as dt
import glob
import hashlib
import json
import math
import os
import random
import subprocess
import sys
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "data")
OUT = os.path.join(HERE, "out")
ASSET = os.path.normpath(os.path.join(HERE, "..", "..", "assets", "request_classifier.json"))
SEED = 13

CLASSES = [
    "code_generation", "code_understanding", "technical_design", "analytical_reasoning",
    "writing", "factual_lookup", "general",
]
ABBREV = {"cg": "code_generation", "cu": "code_understanding", "td": "technical_design",
          "ar": "analytical_reasoning", "wr": "writing", "fl": "factual_lookup", "ge": "general"}
TAG_ALIASES = {"neg": "negation", "mis": "misleading_keyword", "ne": "non_english",
               "multi": "multi_intent", "very_short": "very_short"}
NUM_BUCKETS = 1 << 15
NUM_DENSE = 16
FEATURE_ENGINE = "rc-v1"
SCHEMA = "nasiko-request-classifier-weights-v1"

PAD_PREFIX = ["Hi!", "Hello there,", "Hey team,", "Good morning!", "Hi, hope you're well.",
              "Quick question:", "Hey, sorry to bother you,", "Hello! Thanks in advance for the help."]
PAD_SUFFIX = ["Thanks!", "Thanks so much.", "Cheers,\nSam", "Appreciate it!", "Thank you in advance.",
              "Best regards,\nPriya", "ty!", "Thanks a lot, really stuck on this."]
FILLERS = ["basically", "like", "um", "kind of", "honestly", "just"]
PLACEHOLDERS = ["No other files or changes needed.", "No codebase context.",
                "No additional context.", "Context: none provided."]


def read_jsonl(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def write_jsonl(path, rows):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False, sort_keys=True) + "\n")


def stable_rng(*parts) -> random.Random:
    h = hashlib.sha256(("|".join(str(p) for p in parts) + f"|{SEED}").encode()).hexdigest()
    return random.Random(int(h[:16], 16))


# ----------------------------------------------------------------------------- build

def add_typos(text: str, rng: random.Random, rate=0.06) -> str:
    words = text.split(" ")
    out = []
    for w in words:
        if len(w) > 3 and w.isalpha() and rng.random() < rate:
            i = rng.randrange(len(w) - 1)
            w = w[:i] + w[i + 1] + w[i] + w[i + 2:]
        out.append(w)
    return " ".join(out)


def variant(item, kind, rng):
    q, ctx = item["query"], item["context"]
    tags = set(item["tags"])
    if kind == "padded":
        q = f"{rng.choice(PAD_PREFIX)} {q}\n\n{rng.choice(PAD_SUFFIX)}"
        tags.add("padded")
    elif kind == "noisy":
        noisy = add_typos(q, rng)
        if noisy == q:
            # Guarantee a real perturbation: one transposition, else a filler word.
            words = q.split(" ")
            eligible = [i for i, w in enumerate(words) if len(w) > 3 and w.isalpha()]
            if eligible:
                i = rng.choice(eligible)
                w = words[i]
                j = rng.randrange(len(w) - 1)
                words[i] = w[:j] + w[j + 1] + w[j] + w[j + 2:]
            else:
                words.insert(rng.randrange(len(words) + 1), rng.choice(FILLERS))
            noisy = " ".join(words)
        q = noisy
        if rng.random() < 0.5:
            q = q.lower()
        if rng.random() < 0.5:
            q = q.rstrip(".?!")
        if rng.random() < 0.4:
            words = q.split(" ")
            words.insert(rng.randrange(len(words) + 1), rng.choice(FILLERS))
            q = " ".join(words)
        tags.add("noisy")
    elif kind == "placeholder":
        ctx = rng.choice(PLACEHOLDERS)
        tags.add("placeholder_context")
    return q, ctx, sorted(tags)


def auto_tags(query, context, tags):
    t = set(tags)
    if len(query.split()) <= 6:
        t.add("very_short")
    if context and len(context) >= 800:
        t.add("long_context")
    return sorted(t)


def cmd_build(args):
    # Train-only files (error-analysis iterations) are built last under their own id prefix, so
    # adding one never renumbers — and thus never re-splits — the original scenarios.
    files = sorted(glob.glob(os.path.join(DATA, "base", "*.jsonl")))
    files = [f for f in files if not f.endswith("_train_only.jsonl")] + \
            [f for f in files if f.endswith("_train_only.jsonl")]
    counters = Counter()
    rows = []
    for path in files:
        prefix = "it-" if path.endswith("_train_only.jsonl") else ""
        for raw in read_jsonl(path):
            rt = ABBREV[raw["t"]]
            counters[prefix + raw["t"]] += 1
            base_id = f"{prefix}{raw['t']}-{counters[prefix + raw['t']]:04d}"
            tags = sorted({TAG_ALIASES.get(t, t) for t in raw.get("tags", [])})
            item = {
                "id": base_id, "group_id": raw.get("g", base_id), "query": raw["q"],
                "context": raw.get("x"), "request_type": rt, "complexity": int(raw["c"]),
                "slice": raw["s"], "source": "llm-a", "adjudicated": False,
                "train_only": bool(raw.get("train_only", False)),
            }
            item["tags"] = auto_tags(item["query"], item["context"], tags)
            rows.append(item)
            rng = stable_rng(base_id)
            kinds = [rng.choice(["padded", "noisy"])]
            if rng.random() < 0.5:
                kinds.append("placeholder" if not item["context"] else
                             ("noisy" if kinds[0] == "padded" else "padded"))
            for n, kind in enumerate(kinds, 1):
                q, ctx, vtags = variant(item, kind, stable_rng(base_id, kind))
                rows.append({**item, "id": f"{base_id}-v{n}", "query": q, "context": ctx,
                             "tags": auto_tags(q, ctx, vtags), "source": f"augment:{kind}"})
    write_jsonl(os.path.join(DATA, "raw.jsonl"), rows)
    print(f"build: {len(rows)} items ({sum(counters.values())} base scenarios) -> data/raw.jsonl")


# ----------------------------------------------------------------------------- split

def norm_text(s: str) -> str:
    return " ".join("".join(ch.lower() if ch.isalnum() else " " for ch in s).split())


def shingles(s: str, n=5):
    s = norm_text(s)
    return {s[i:i + n] for i in range(max(1, len(s) - n + 1))}


def jaccard(a, b):
    return len(a & b) / max(1, len(a | b))


def cmd_split(args):
    rows = read_jsonl(os.path.join(DATA, "raw.jsonl"))
    public = []
    if args.public and os.path.exists(args.public):
        public = [shingles(e["query"]) for e in json.load(open(args.public))["examples"]]
    # Public-set guard + exact dedup on normalized text.
    seen, kept, dropped_public, dropped_dup = set(), [], 0, 0
    for r in rows:
        sh = shingles(r["query"])
        if any(jaccard(sh, p) >= 0.5 for p in public):
            dropped_public += 1
            continue
        key = norm_text(r["query"]) + "\x1f" + norm_text(r["context"] or "")
        if key in seen:
            dropped_dup += 1
            continue
        seen.add(key)
        kept.append(r)
    # Group split stratified by request type.
    groups = defaultdict(list)
    for r in kept:
        groups[r["group_id"]].append(r)
    by_type = defaultdict(list)
    assign = {}
    for gid, items in groups.items():
        if items[0].get("train_only"):
            assign[gid] = "train"  # error-analysis additions never enter val/test
        else:
            by_type[items[0]["request_type"]].append(gid)
    for rt in CLASSES:
        gids = sorted(by_type[rt])
        random.Random(f"{SEED}:{rt}").shuffle(gids)
        n = len(gids)
        n_val, n_test = round(n * 0.15), round(n * 0.15)
        for i, gid in enumerate(gids):
            assign[gid] = "val" if i < n_val else "test" if i < n_val + n_test else "train"
    # Cross-split near-duplicate guard: move offending val/test groups into train.
    sh = {r["id"]: shingles(r["query"] + " " + (r["context"] or "")) for r in kept}
    train_ids = [r["id"] for r in kept if assign[r["group_id"]] == "train"]
    moved = 0
    for r in kept:
        if assign[r["group_id"]] == "train":
            continue
        if any(jaccard(sh[r["id"]], sh[t]) >= 0.6 for t in train_ids):
            assign[r["group_id"]] = "train"
            moved += 1
    for r in kept:
        r["split"] = assign[r["group_id"]]
    # Assert no cross-split near-dups remain (val/test vs train and val vs test).
    by_split = defaultdict(list)
    for r in kept:
        by_split[r["split"]].append(r["id"])
    for a, b in [("val", "train"), ("test", "train"), ("val", "test")]:
        for x in by_split[a]:
            for y in by_split[b]:
                assert jaccard(sh[x], sh[y]) < 0.6, f"near-dup across {a}/{b}: {x} {y}"
    write_jsonl(os.path.join(DATA, "labelled.jsonl"), kept)
    for split in ("val", "test"):
        examples = [{"id": r["id"], "query": r["query"], "context": r["context"],
                     "request_type": r["request_type"], "complexity": r["complexity"],
                     "tests": [r["slice"]] + r["tags"]} for r in kept if r["split"] == split]
        with open(os.path.join(DATA, f"eval_{split}.json"), "w", encoding="utf-8") as f:
            json.dump({"schema_version": "h4-public-eval-v1",
                       "purpose": f"P2 own held-out split ({split}); synthetic, never trained on",
                       "source": "llm-router/training/request_classifier",
                       "examples": examples}, f, ensure_ascii=False, indent=1)
    c = Counter(r["split"] for r in kept)
    print(f"split: kept {len(kept)} (dropped {dropped_public} public-near-dup, {dropped_dup} exact dup); "
          f"moved {moved} near-dup items to train; {dict(c)}")
    for split in ("train", "val", "test"):
        print(f"  {split}: " + ", ".join(f"{rt}={sum(1 for r in kept if r['split']==split and r['request_type']==rt)}"
                                         for rt in CLASSES))


# ----------------------------------------------------------------------------- train

def load_features(path):
    import numpy as np
    from scipy.sparse import csr_matrix
    recs = read_jsonl(path)
    rows, cols, vals = [], [], []
    for i, r in enumerate(recs):
        for b, v in r["hashed"]:
            rows.append(i); cols.append(b); vals.append(v)
        for j, v in enumerate(r["dense"]):
            if v != 0.0:
                rows.append(i); cols.append(NUM_BUCKETS + j); vals.append(v)
    X = csr_matrix((np.array(vals, dtype=np.float64), (rows, cols)),
                   shape=(len(recs), NUM_BUCKETS + NUM_DENSE))
    y = np.array([CLASSES.index(r["y_type"]) for r in recs])
    cx = np.array([int(r["y_cx"]) for r in recs])
    split = np.array([r["split"] for r in recs])
    meta = recs
    return X, y, cx, split, meta


def softmax(Z):
    import numpy as np
    Z = Z - Z.max(axis=1, keepdims=True)
    E = np.exp(Z)
    return E / E.sum(axis=1, keepdims=True)


def type_logits(model, X):
    """Logits in CLASSES order from a fitted sklearn LR whose classes_ are indices."""
    import numpy as np
    Z = X @ model["W"].T + model["b"]
    return np.asarray(Z)


def cx_predict(heads, X):
    import numpy as np
    q_prev = np.ones(X.shape[0])
    out = np.ones(X.shape[0], dtype=int)
    for w, b in heads:
        z = np.asarray(X @ w).ravel() + b
        q = np.minimum(1 / (1 + np.exp(-z)), q_prev)
        q_prev = q
        out += (q >= 0.5).astype(int)
    return out


def fit_type_head(X, y, C):
    import numpy as np
    from sklearn.linear_model import LogisticRegression
    lr = LogisticRegression(C=C, max_iter=5000, tol=1e-6)
    lr.fit(X, y)
    W = np.zeros((len(CLASSES), X.shape[1])); b = np.zeros(len(CLASSES))
    for row, cls in enumerate(lr.classes_):
        W[cls] = lr.coef_[row]; b[cls] = lr.intercept_[row]
    return {"W": W, "b": b}


def fit_cx_heads(X, cx, C):
    import numpy as np
    from sklearn.linear_model import LogisticRegression
    heads = []
    for k in range(1, 5):
        target = (cx > k).astype(int)
        if target.min() == target.max():
            heads.append((np.zeros(X.shape[1]), 8.0 if target[0] == 1 else -8.0))
            continue
        lr = LogisticRegression(C=C, max_iter=5000, tol=1e-6)
        lr.fit(X, target)
        heads.append((lr.coef_[0].copy(), float(lr.intercept_[0])))
    return heads


def nll(P, y):
    import numpy as np
    return float(-np.mean(np.log(np.clip(P[np.arange(len(y)), y], 1e-12, 1.0))))


def group_folds(meta, mask, k=5):
    """Deterministic group k-fold over the masked rows: all variants of a scenario share a fold."""
    import numpy as np
    idx = np.where(mask)[0]
    groups = sorted({meta[i]["id"].rsplit("-v", 1)[0] for i in idx})
    random.Random(f"{SEED}:folds").shuffle(groups)
    fold_of = {g: n % k for n, g in enumerate(groups)}
    fold = np.array([fold_of[meta[i]["id"].rsplit("-v", 1)[0]] for i in idx])
    return idx, fold


def cmd_train(args):
    """Select C and the temperature by group 5-fold cross-validation on the train split only
    (out-of-fold predictions), then fit the final heads on all of train. Validation is never used
    for selection, so it stays an honest estimate."""
    import numpy as np
    from scipy.optimize import minimize_scalar
    X, y, cx, split, meta = load_features(args.features)
    tr = split == "train"
    idx, fold = group_folds(meta, tr)
    grid = [0.3, 1.0, 3.0, 10.0, 30.0, 100.0]
    best = None
    for C in grid:
        oof = np.zeros((len(idx), len(CLASSES)))
        for f in range(5):
            fit_rows, hold = idx[fold != f], fold == f
            m = fit_type_head(X[fit_rows], y[fit_rows], C)
            oof[hold] = type_logits(m, X[idx[hold]])
        loss = nll(softmax(oof), y[idx])
        acc = float((oof.argmax(1) == y[idx]).mean())
        print(f"  type head C={C:<5} CV NLL={loss:.4f} CV acc={acc:.3f}")
        if best is None or loss < best[0]:
            best = (loss, C, oof)
    _, C_type, oof_best = best
    res = minimize_scalar(lambda T: nll(softmax(oof_best / T), y[idx]), bounds=(0.3, 5.0),
                          method="bounded", options={"xatol": 1e-4})
    T = float(res.x)
    type_model = fit_type_head(X[tr], y[tr], C_type)
    best_cx = None
    for C in grid:
        pred = np.zeros(len(idx), dtype=int)
        for f in range(5):
            fit_rows, hold = idx[fold != f], fold == f
            heads = fit_cx_heads(X[fit_rows], cx[fit_rows], C)
            pred[hold] = cx_predict(heads, X[idx[hold]])
        mae = float(np.abs(pred - cx[idx]).mean())
        print(f"  cx heads  C={C:<5} CV MAE={mae:.3f}")
        if best_cx is None or mae < best_cx[0]:
            best_cx = (mae, C)
    _, C_cx = best_cx
    heads = fit_cx_heads(X[tr], cx[tr], C_cx)
    # Regex constants: accuracy of the regex on val, split by whether any pattern fired.
    va = split == "val"
    regex = {"matched": [], "defaulted": []}
    for r, keep in zip(meta, va):
        if keep:
            regex["matched" if r["regex_votes_total"] > 0 else "defaulted"].append(r["regex_type"] == r["y_type"])
    rc = {k: (sum(v) / len(v) if v else 0.0) for k, v in regex.items()}
    os.makedirs(OUT, exist_ok=True)
    np.savez(os.path.join(OUT, "model.npz"), W=type_model["W"], b=type_model["b"], T=T,
             cxW=np.stack([w for w, _ in heads]), cxb=np.array([b for _, b in heads]),
             C_type=C_type, C_cx=C_cx, regex_matched=rc["matched"], regex_defaulted=rc["defaulted"],
             regex_n=np.array([len(regex["matched"]), len(regex["defaulted"])]))
    print(f"train: C_type={C_type} C_cx={C_cx} T={T:.4f} (CV); regex val accuracy matched={rc['matched']:.3f} "
          f"(n={len(regex['matched'])}) defaulted={rc['defaulted']:.3f} (n={len(regex['defaulted'])})")


def load_model():
    import numpy as np
    z = np.load(os.path.join(OUT, "model.npz"))
    heads = [(z["cxW"][k], float(z["cxb"][k])) for k in range(4)]
    return z, {"W": z["W"], "b": z["b"]}, float(z["T"]), heads


# ----------------------------------------------------------------------------- metrics

def ece(conf, correct, bins=10):
    rows, total, n = [], 0.0, len(conf)
    for i in range(bins):
        lo, hi = i / bins, (i + 1) / bins
        idx = [j for j, c in enumerate(conf) if (lo < c <= hi) or (i == 0 and c == 0.0)]
        if not idx:
            rows.append((lo, hi, 0, None, None))
            continue
        acc = sum(correct[j] for j in idx) / len(idx)
        avg = sum(conf[j] for j in idx) / len(idx)
        total += len(idx) / n * abs(acc - avg)
        rows.append((lo, hi, len(idx), acc, avg))
    return total, rows


def metrics_md(name, gold, pred, conf, gold_cx, pred_cx, tests=None, lat=None):
    n = len(gold)
    correct = [g == p for g, p in zip(gold, pred)]
    acc = sum(correct) / n
    f1s, lines = [], []
    for c in CLASSES:
        tp = sum(1 for g, p in zip(gold, pred) if g == p == c)
        fp = sum(1 for g, p in zip(gold, pred) if p == c and g != c)
        fn = sum(1 for g, p in zip(gold, pred) if g == c and p != c)
        prec = tp / (tp + fp) if tp + fp else 0.0
        rec = tp / (tp + fn) if tp + fn else 0.0
        f1 = 2 * prec * rec / (prec + rec) if prec + rec else 0.0
        f1s.append(f1)
        lines.append(f"| {c} | {tp + fn} | {prec:.3f} | {rec:.3f} | {f1:.3f} |")
    e, bins = ece(conf, correct)
    brier = sum((c - k) ** 2 for c, k in zip(conf, correct)) / n
    diffs = [abs(a - b) for a, b in zip(gold_cx, pred_cx)]
    out = [f"### {name}", "",
           f"n={n} · **type accuracy {acc:.3f}** · macro-F1 {sum(f1s)/len(f1s):.3f} · ECE(10) {e:.3f} · "
           f"top-label Brier {brier:.3f} · complexity exact {sum(d == 0 for d in diffs)/n:.3f}, "
           f"±1 {sum(d <= 1 for d in diffs)/n:.3f}, MAE {sum(diffs)/n:.3f}"]
    if lat:
        s = sorted(lat)
        out[-1] += f" · latency p50 {s[len(s)//2]}µs p95 {s[min(len(s)-1, int(0.95*(len(s)-1)+0.5))]}µs"
    out += ["", "| class | support | precision | recall | F1 |", "|---|---|---|---|---|"] + lines
    out += ["", "Confusion (rows = gold, cols = predicted, order as above):", "",
            "| gold \\ pred | " + " | ".join(c[:6] for c in CLASSES) + " |",
            "|---|" + "---|" * len(CLASSES)]
    for g in CLASSES:
        out.append(f"| {g[:12]} | " + " | ".join(str(sum(1 for a, b in zip(gold, pred) if a == g and b == p))
                                                 for p in CLASSES) + " |")
    out += ["", "Reliability (ECE bins): range · count · accuracy · mean confidence", ""]
    for lo, hi, cnt, a, m in bins:
        if cnt:
            out.append(f"- ({lo:.1f}, {hi:.1f}] · {cnt} · {a:.3f} · {m:.3f}")
    out += ["", "Selective accuracy (abstain below τ):", "", "| τ | coverage | accuracy on accepted |", "|---|---|---|"]
    for tau in [0.0, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9]:
        idx = [j for j, c in enumerate(conf) if c >= tau]
        if idx:
            out.append(f"| {tau:.1f} | {len(idx)/n:.3f} | {sum(correct[j] for j in idx)/len(idx):.3f} |")
    if tests:
        groups = defaultdict(list)
        for t, ok in zip(tests, correct):
            for tag in t:
                groups[tag].append(ok)
        out += ["", "Per slice / tag accuracy:", "", "| slice/tag | n | accuracy |", "|---|---|---|"]
        for tag in sorted(groups):
            v = groups[tag]
            out.append(f"| {tag} | {len(v)} | {sum(v)/len(v):.3f} |")
    return "\n".join(out) + "\n"


def suggest_tau(conf, correct, target=0.85):
    for tau in [x / 20 for x in range(0, 20)]:
        idx = [j for j, c in enumerate(conf) if c >= tau]
        if idx and sum(correct[j] for j in idx) / len(idx) >= target:
            return tau, len(idx) / len(conf)
    return None, 0.0


def cmd_eval(args):
    import numpy as np
    X, y, cx, split, meta = load_features(args.features)
    z, tm, T, heads = load_model()
    va = split == args.split
    labelled = {r["id"]: r for r in read_jsonl(os.path.join(DATA, "labelled.jsonl"))}
    tests = [[labelled[r["id"]]["slice"]] + labelled[r["id"]]["tags"] for r, k in zip(meta, va) if k]
    gold = [CLASSES[i] for i in y[va]]
    gcx = list(cx[va])
    Z = type_logits(tm, X[va])
    pcx = list(cx_predict(heads, X[va]))
    rx_pred = [r["regex_type"] for r, k in zip(meta, va) if k]
    rx_conf = [float(z["regex_matched"]) if r["regex_votes_total"] > 0 else float(z["regex_defaulted"])
               for r, k in zip(meta, va) if k]
    md = [f"# Validation report ({args.split})", ""]
    md.append(metrics_md("regex (fixed confidence constants, complexity 3)", gold, rx_pred, rx_conf, gcx,
                         [3] * len(gold), tests))
    for label, P in [("local raw (T=1)", softmax(Z)), (f"local + temperature (T={T:.3f})", softmax(Z / T))]:
        pred = [CLASSES[i] for i in P.argmax(1)]
        conf = [float(round(p, 4)) for p in P.max(1)]
        md.append(metrics_md(label, gold, pred, conf, gcx, pcx, tests))
    P = softmax(Z / T)
    correct = [CLASSES[i] == g for i, g in zip(P.argmax(1), gold)]
    tau, cov = suggest_tau(list(P.max(1)), correct)
    md.append(f"Suggested τ (accuracy on accepted ≥ 0.85 at max coverage): {tau} (coverage {cov:.3f})\n")
    text = "\n".join(md)
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, f"{args.split}_report.md"), "w") as f:
        f.write(text)
    print(text)


def sig5(x: float) -> float:
    return float(f"{x:.5g}")


def cmd_export(args):
    import numpy as np
    z, tm, T, heads = load_model()
    W, b = tm["W"], tm["b"]
    cxW = np.stack([w for w, _ in heads]); cxb = np.array([bb for _, bb in heads])
    hashed = {}
    threshold = args.prune
    while True:
        hashed = {}
        for bucket in range(NUM_BUCKETS):
            row = list(W[:, bucket]) + list(cxW[:, bucket])
            if max(abs(v) for v in row) >= threshold:
                hashed[str(bucket)] = [sig5(v) for v in row]
        size_est = len(json.dumps(hashed, separators=(",", ":")))
        if size_est <= 1_900_000:
            break
        threshold *= 1.5
        print(f"export: artifact too large ({size_est} bytes); raising prune threshold to {threshold:g}")
    labelled_path = os.path.join(DATA, "labelled.jsonl")
    dataset_sha = hashlib.sha256(open(labelled_path, "rb").read()).hexdigest()
    try:
        git_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=HERE, text=True).strip()
    except Exception:
        git_sha = ""
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    trained_at = (dt.datetime.fromtimestamp(int(epoch), dt.timezone.utc) if epoch
                  else dt.datetime.now(dt.timezone.utc)).strftime("%Y-%m-%dT%H:%M:%SZ")
    feats = read_jsonl(args.features)
    n_train = sum(1 for r in feats if r["split"] == "train")
    n_val = sum(1 for r in feats if r["split"] == "val")
    art = {
        "schema": SCHEMA, "feature_engine": FEATURE_ENGINE, "num_buckets": NUM_BUCKETS,
        "num_dense_features": NUM_DENSE, "classes": CLASSES,
        "type_bias": [sig5(v) for v in b],
        "type_dense": [[sig5(v) for v in W[k, NUM_BUCKETS:]] for k in range(len(CLASSES))],
        "temperature": sig5(T),
        "cx_bias": [sig5(v) for v in cxb],
        "cx_dense": [[sig5(v) for v in cxW[k, NUM_BUCKETS:]] for k in range(4)],
        "hashed": hashed,
        "provenance": {
            "trained_at_utc": trained_at, "dataset_sha256": dataset_sha, "git_sha": git_sha, "seed": SEED,
            "C_type": float(z["C_type"]), "C_cx": float(z["C_cx"]), "n_train": n_train, "n_val": n_val,
            "prune_threshold": threshold,
            "regex_val_accuracy_matched": round(float(z["regex_matched"]), 4),
            "regex_val_accuracy_defaulted": round(float(z["regex_defaulted"]), 4),
            "trainer": "training/request_classifier/rc.py (scikit-learn LogisticRegression)",
        },
    }
    text = json.dumps(art, separators=(",", ":"), sort_keys=False) + "\n"
    with open(ASSET, "w") as f:
        f.write(text)
    sha = hashlib.sha256(text.encode()).hexdigest()
    print(f"export: {ASSET} ({len(text)} bytes, {len(hashed)} buckets, prune {threshold:g}) sha256={sha}")
    print(f"  regex constants for classifier.rs: REGEX_CONFIDENCE_MATCHED={float(z['regex_matched']):.2f} "
          f"REGEX_CONFIDENCE_DEFAULTED={float(z['regex_defaulted']):.2f} (n={list(z['regex_n'])})")


# ----------------------------------------------------------------------------- report / kappa

def cmd_report(args):
    data = json.load(open(args.eval))
    ex = {e["id"]: e for e in data["examples"]}

    def section(path, name):
        outs = read_jsonl(path)
        outs = [o for o in outs if o["id"] in ex]
        gold = [ex[o["id"]]["request_type"] for o in outs]
        pred = [o["request_type"] for o in outs]
        conf = [float(o["confidence"]) for o in outs]
        gcx = [int(ex[o["id"]]["complexity"]) for o in outs]
        pcx = [int(o["complexity"]) for o in outs]
        tests = [ex[o["id"]].get("tests", []) for o in outs]
        lat = [int(o["latency_us"]) for o in outs]
        return metrics_md(name, gold, pred, conf, gcx, pcx, tests, lat), dict(zip([o["id"] for o in outs], pred))

    md, preds_a = section(args.out, args.name or os.path.basename(args.out))
    text = [f"# Report on {os.path.basename(args.eval)}", "", md]
    if args.out_b:
        md_b, preds_b = section(args.out_b, args.name_b or os.path.basename(args.out_b))
        text.append(md_b)
        wins_a, wins_b = Counter(), Counter()
        for i, e in ex.items():
            if i in preds_a and i in preds_b:
                a_ok, b_ok = preds_a[i] == e["request_type"], preds_b[i] == e["request_type"]
                for tag in e.get("tests", []):
                    if a_ok and not b_ok:
                        wins_a[tag] += 1
                    if b_ok and not a_ok:
                        wins_b[tag] += 1
        text.append("Head-to-head wins per slice/tag (A = first OUT, B = second OUT):\n")
        text.append("| slice/tag | A only correct | B only correct |\n|---|---|---|")
        for tag in sorted(set(wins_a) | set(wins_b)):
            text.append(f"| {tag} | {wins_a[tag]} | {wins_b[tag]} |")
    out = "\n".join(text) + "\n"
    if args.write:
        with open(args.write, "w") as f:
            f.write(out)
    print(out)


def cmd_kappa(args):
    a = {r["id"]: r for r in read_jsonl(args.a)}
    b = {r["id"]: r for r in read_jsonl(args.b)}
    ids = sorted(set(a) & set(b))
    from sklearn.metrics import cohen_kappa_score
    kt = cohen_kappa_score([a[i]["request_type"] for i in ids], [b[i]["request_type"] for i in ids])
    kc = cohen_kappa_score([int(a[i]["complexity"]) for i in ids], [int(b[i]["complexity"]) for i in ids],
                           weights="quadratic")
    print(f"kappa on {len(ids)} shared ids: type κ={kt:.3f}, complexity weighted κ={kc:.3f}")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)
    sub.add_parser("build")
    s = sub.add_parser("split"); s.add_argument("--public", default="/tmp/classifier-eval.json")
    feats = os.path.join(OUT, "features.jsonl")
    for name in ("train", "export"):
        s = sub.add_parser(name); s.add_argument("--features", default=feats)
        if name == "export":
            s.add_argument("--prune", type=float, default=1e-3)
    s = sub.add_parser("eval"); s.add_argument("--features", default=feats); s.add_argument("--split", default="val")
    s = sub.add_parser("report")
    s.add_argument("--eval", required=True); s.add_argument("--out", required=True)
    s.add_argument("--out-b"); s.add_argument("--name"); s.add_argument("--name-b"); s.add_argument("--write")
    s = sub.add_parser("kappa"); s.add_argument("--a", required=True); s.add_argument("--b", required=True)
    args = p.parse_args()
    {"build": cmd_build, "split": cmd_split, "train": cmd_train, "eval": cmd_eval, "export": cmd_export,
     "report": cmd_report, "kappa": cmd_kappa}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
