#!/usr/bin/env python3
"""Deterministically split labelled.txt into train.jsonl / val.jsonl.

Format of labelled.txt: request_type|complexity|query   (see LABELLING.md)
 * near-duplicates (word-set Jaccard >= 0.6 against any kept row) are dropped, so no
   paraphrase of a validation row can sit in train;
 * the split is stratified per class (every 4th row by FNV-1a hash order -> val), no RNG.
"""
import json, re, collections

def fnv(s):
    h = 0xcbf29ce484222325
    for b in s.encode():
        h = ((h ^ b) * 0x100000001b3) & 0xFFFFFFFFFFFFFFFF
    return h

def words(s):
    return set(re.findall(r"[a-z0-9]+", s.lower()))

rows, kept_words, dropped = [], [], 0
for line in open("labelled.txt", encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    rt, cx, q = line.split("|", 2)
    w = words(q)
    if any(len(w & o) / max(1, len(w | o)) >= 0.6 for o in kept_words):
        dropped += 1
        continue
    kept_words.append(w)
    rows.append({"request_type": rt, "complexity": int(cx), "query": q})

by = collections.defaultdict(list)
for r in rows:
    by[r["request_type"]].append(r)
train, val = [], []
for rt, rs in sorted(by.items()):
    rs.sort(key=lambda r: fnv(r["query"]))
    for i, r in enumerate(rs):
        (val if i % 4 == 3 else train).append(r)
for name, data in (("train.jsonl", train), ("val.jsonl", val)):
    with open(name, "w", encoding="utf-8") as f:
        for r in sorted(data, key=lambda r: fnv(r["query"])):
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
print(f"train={len(train)} val={len(val)} near_duplicates_dropped={dropped}")

# Eval-format copy of the held-out split (same schema as the public classifier-eval@v1 file).
with open("val_eval.json", "w", encoding="utf-8") as f:
    json.dump({"schema": "classifier-eval@v1-local-heldout", "examples": [
        {"id": f"val-{i:02d}", "query": r["query"], "context": None,
         "request_type": r["request_type"], "complexity": r["complexity"]}
        for i, r in enumerate(sorted(val, key=lambda r: fnv(r["query"])))]}, f, ensure_ascii=False, indent=1)
