"""Pack the Rust-computed features + labels into one compressed .npz for Kaggle.

Features come from the router's own feature engine (`classifier_features` example), so the
weights trained on Kaggle match inference exactly. Run locally:

    python make_features_npz.py --data ../data --features <dir> --out linear_features.npz
"""

import argparse
import hashlib
import json
import pathlib

import numpy as np

LABELS = ["code_generation", "code_understanding", "technical_design", "analytical_reasoning",
          "writing", "factual_lookup", "general"]
NUM_BUCKETS = 1 << 18
NUM_DENSE = 8


def pack(data_dir, feat_dir, split):
    examples = json.loads((data_dir / f"{split}.json").read_text(encoding="utf-8"))["examples"]
    feats = {}
    for line in (feat_dir / f"{split}.features.jsonl").read_text(encoding="utf-8").splitlines():
        row = json.loads(line)
        feats[row["id"]] = row
    indptr, indices, values = [0], [], []
    for ex in examples:
        f = feats[ex["id"]]
        for bucket, value in f["hashed"]:
            indices.append(bucket)
            values.append(value)
        for k, value in enumerate(f["dense"]):  # dense features live after the hashed buckets
            if value != 0.0:
                indices.append(NUM_BUCKETS + k)
                values.append(value)
        indptr.append(len(indices))
    return {
        f"{split}_indptr": np.array(indptr, dtype=np.int64),
        f"{split}_indices": np.array(indices, dtype=np.int32),
        f"{split}_values": np.array(values, dtype=np.float32),
        f"{split}_type": np.array([LABELS.index(e["request_type"]) for e in examples], dtype=np.int8),
        f"{split}_complexity": np.array([e["complexity"] - 1 for e in examples], dtype=np.int8),
        f"{split}_group": np.array([e["group"] for e in examples]),
        f"{split}_id": np.array([e["id"] for e in examples]),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--features", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    data_dir, feat_dir = pathlib.Path(args.data), pathlib.Path(args.features)
    arrays = {**pack(data_dir, feat_dir, "train"), **pack(data_dir, feat_dir, "val")}
    arrays["labels"] = np.array(LABELS)
    arrays["num_buckets"] = np.array(NUM_BUCKETS)
    arrays["num_dense"] = np.array(NUM_DENSE)
    arrays["train_sha256"] = np.array(hashlib.sha256((data_dir / "train.json").read_bytes()).hexdigest())
    np.savez_compressed(args.out, **arrays)
    out = pathlib.Path(args.out)
    print(f"wrote {out} ({out.stat().st_size / 1e6:.1f} MB): "
          f"{len(arrays['train_id'])} train / {len(arrays['val_id'])} val")


if __name__ == "__main__":
    main()
