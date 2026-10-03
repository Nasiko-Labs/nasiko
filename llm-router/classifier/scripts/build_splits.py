"""Build leakage-free train/validation splits for the request classifier.

Reads raw labelled examples (JSON Lines, one file per source), validates them against the
labelling schema, drops anything resembling the published eval sample (and asserts none
survives into either split, so no public case is trained or tuned on), merges
near-duplicate groups, and splits by group so no paraphrase family or near-duplicate pair
spans train and validation. See ../LABELLING.md section 4.

    python build_splits.py --raw <dir of *.jsonl> --public classifier-eval.json --out ../data

Output files use the eval-set schema (an object with an ``examples`` array), so
``classifier_eval`` runs on ``val.json`` unchanged.
"""

import argparse
import json
import pathlib
import random
from collections import Counter, defaultdict

import numpy as np
from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.metrics.pairwise import cosine_similarity

LABELS = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
]
KEYS = {"query", "context", "request_type", "complexity", "tests", "group"}
NEAR_DUPLICATE = 0.85  # cosine at or above this joins two groups
PUBLIC_OVERLAP = 0.60  # cosine at or above this to a public eval case drops the example
VAL_FRACTION = 0.12
SEED = 13
CONTEXT_CHARS_FOR_SIMILARITY = 300


def load_raw(raw_dir):
    rows, rejected = [], Counter()
    for path in sorted(pathlib.Path(raw_dir).glob("*.jsonl")):
        for line in path.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                rejected["bad_json"] += 1
                continue
            problem = validate(row)
            if problem:
                rejected[problem] += 1
                continue
            row["query"] = row["query"].strip()
            row["context"] = (row.get("context") or "").strip()
            rows.append(row)
    return rows, rejected


def validate(row):
    if set(row) != KEYS:
        return "bad_keys"
    if row["request_type"] not in LABELS:
        return "bad_label"
    if not isinstance(row["complexity"], int) or not 1 <= row["complexity"] <= 5:
        return "bad_complexity"
    if not isinstance(row["query"], str) or not row["query"].strip():
        return "empty_query"
    return None


def similarity_text(row):
    return row["query"] + " " + row["context"][:CONTEXT_CHARS_FOR_SIMILARITY]


def drop_exact_duplicates(rows):
    seen, kept = set(), []
    for row in rows:
        key = (row["query"].lower(), row["context"].lower())
        if key not in seen:
            seen.add(key)
            kept.append(row)
    return kept


def load_public(public_path):
    return json.loads(pathlib.Path(public_path).read_text(encoding="utf-8"))["examples"]


def public_similarity(rows, public, vectorizer):
    """Per (row, public case): the larger of query+context and query-only cosine similarity.

    Query-only matters because a public case's long context can dilute the combined score of
    a row that copies its query.
    """
    public_full = [similarity_text({"query": p["query"], "context": p.get("context") or ""}) for p in public]
    full = cosine_similarity(
        vectorizer.transform([similarity_text(r) for r in rows]), vectorizer.transform(public_full)
    )
    query_only = cosine_similarity(
        vectorizer.transform([r["query"] for r in rows]), vectorizer.transform([p["query"] for p in public])
    )
    return np.maximum(full, query_only)


def drop_public_overlap(rows, public, vectorizer):
    """Remove every example resembling a published eval case, so none is trained or tuned on."""
    public_queries = {normalise(p["query"]) for p in public}
    sims = public_similarity(rows, public, vectorizer)
    keep = (sims.max(axis=1) < PUBLIC_OVERLAP) & np.array([normalise(r["query"]) not in public_queries for r in rows])
    return [r for r, k in zip(rows, keep) if k], int((~keep).sum())


def assert_public_excluded(splits, public, vectorizer):
    """Fail the build if any split example resembles a public eval case; report the closest."""
    rows = [r for split in splits for r in split]
    sims = public_similarity(rows, public, vectorizer)
    print("closest dataset example to each public eval case (must stay below "
          f"{PUBLIC_OVERLAP}):")
    for j, p in enumerate(public):
        i = int(sims[:, j].argmax())
        print(f"  {p['id']}: {sims[i, j]:.3f}  {rows[i]['query'][:70]!r}")
    worst = float(sims.max())
    assert worst < PUBLIC_OVERLAP, f"public eval case leaked into the data (cosine {worst:.3f})"


def normalise(text):
    return " ".join(text.lower().split())


def merge_near_duplicate_groups(rows, vectorizer):
    """Union-find over groups: any cross-group pair at or above NEAR_DUPLICATE joins them."""
    parent = {r["group"]: r["group"] for r in rows}

    def find(g):
        while parent[g] != g:
            parent[g] = parent[parent[g]]
            g = parent[g]
        return g

    sims = cosine_similarity(vectorizer.transform([similarity_text(r) for r in rows]))
    merges = 0
    for i, j in zip(*((sims >= NEAR_DUPLICATE).nonzero())):
        if i < j:
            a, b = find(rows[i]["group"]), find(rows[j]["group"])
            if a != b:
                parent[b] = a
                merges += 1
    for r in rows:
        r["split_group"] = find(r["group"])
    return merges, sims


def split_groups(rows):
    by_group = defaultdict(list)
    for r in rows:
        by_group[r["split_group"]].append(r)
    by_label = defaultdict(list)
    for g, members in by_group.items():
        majority = Counter(m["request_type"] for m in members).most_common(1)[0][0]
        by_label[majority].append(g)
    rng = random.Random(SEED)
    val_groups = set()
    for label in LABELS:
        groups = sorted(by_label[label])
        rng.shuffle(groups)
        val_groups.update(groups[: round(len(groups) * VAL_FRACTION)])
    train = [r for r in rows if r["split_group"] not in val_groups]
    val = [r for r in rows if r["split_group"] in val_groups]
    return train, val


def assert_no_leakage(rows, sims, train, val):
    index = {id(r): i for i, r in enumerate(rows)}
    ti = [index[id(r)] for r in train]
    vi = [index[id(r)] for r in val]
    worst = sims[vi][:, ti].max()
    assert worst < NEAR_DUPLICATE, f"cross-split near-duplicate at cosine {worst:.3f}"
    return float(worst)


def write_split(path, rows, prefix):
    examples = []
    for n, r in enumerate(rows, 1):
        examples.append(
            {
                "id": f"{prefix}-{n:05d}",
                "query": r["query"],
                "context": r["context"],
                "request_type": r["request_type"],
                "complexity": r["complexity"],
                "tests": r["tests"],
                "group": r["split_group"],
            }
        )
    doc = {
        "schema_version": "nasiko-classifier-data-v1",
        "source": "Synthetic requests written for this classifier and labelled per LABELLING.md; no user data.",
        "labels": {"request_type": LABELS, "complexity": "1-5, rubric in LABELLING.md"},
        "examples": examples,
    }
    path.write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n")


def report(name, rows):
    labels = Counter(r["request_type"] for r in rows)
    complexity = Counter(r["complexity"] for r in rows)
    tags = Counter(t for r in rows for t in r["tests"])
    print(f"{name}: {len(rows)} examples, {len({r['split_group'] for r in rows})} groups")
    print("  labels:", dict(sorted(labels.items())))
    print("  complexity:", dict(sorted(complexity.items())))
    print("  tags:", dict(tags.most_common()))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--raw", required=True)
    ap.add_argument("--public", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    rows, rejected = load_raw(args.raw)
    print(f"loaded {len(rows)} valid rows; rejected {dict(rejected)}")
    rows = drop_exact_duplicates(rows)
    vectorizer = TfidfVectorizer(analyzer="char_wb", ngram_range=(3, 5), sublinear_tf=True)
    vectorizer.fit([similarity_text(r) for r in rows])
    public = load_public(args.public)
    rows, dropped = drop_public_overlap(rows, public, vectorizer)
    print(f"after exact-dup and public-overlap removal: {len(rows)} (public overlap dropped {dropped})")
    merges, sims = merge_near_duplicate_groups(rows, vectorizer)
    print(f"near-duplicate group merges: {merges}")
    train, val = split_groups(rows)
    worst = assert_no_leakage(rows, sims, train, val)
    print(f"max train/val cosine: {worst:.3f} (< {NEAR_DUPLICATE})")
    assert_public_excluded([train, val], public, vectorizer)

    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    write_split(out / "train.json", train, "tr")
    write_split(out / "val.json", val, "va")
    report("train", train)
    report("val", val)


if __name__ == "__main__":
    main()
