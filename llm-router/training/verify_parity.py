"""
Verify Python and Rust feature parity and generate a fixture file for Rust testing.
"""

import json
import math
import numpy as np
from features import (
    extract_full_features,
    p2_dense_features,
    regex_vote_features,
    p2_hashed_features,
    classify_request_type,
    NUM_BUCKETS,
)

CASES = [
    {
        "id": "case1",
        "query": "write a python function to parse json",
        "context": None,
    },
    {
        "id": "case2",
        "query": "Explain why this function is slow and suggest optimizations.",
        "context": "Here is the slow code: for i in range(1000): db.query(i)",
    },
    {
        "id": "case3",
        "query": "Draft a polite email asking for project updates.",
        "context": None,
    },
    {
        "id": "case4",
        "query": "What is the capital of France?",
        "context": None,
    },
    {
        "id": "case5",
        "query": "Calculate the compound interest for $1000 at 5% over 10 years.",
        "context": None,
    },
]

def softmax(x):
    e = np.exp(x - np.max(x))
    return (e / e.sum()).tolist()

def main():
    fixtures = []
    for c in CASES:
        q = c["query"]
        ctx = c["context"]
        hashed = p2_hashed_features(q, ctx)
        dense = p2_dense_features(q, ctx)
        regex_votes = regex_vote_features(q)
        rt = classify_request_type(q)

        # Sort hashed items by bucket id
        sorted_hashed = sorted(hashed.items(), key=lambda x: x[0])
        sample_hashed = [{"bucket": b, "val": round(v, 8)} for b, v in sorted_hashed[:10]]

        fixtures.append({
            "id": c["id"],
            "query": q,
            "context": ctx,
            "regex_type": rt,
            "dense": [round(x, 8) for x in dense],
            "regex_votes": regex_votes,
            "num_hashed_buckets": len(hashed),
            "sum_hashed": round(sum(hashed.values()), 8),
            "sample_hashed": sample_hashed,
        })

    with open("llm-router/training/parity_fixtures.json", "w", encoding="utf-8") as f:
        json.dump(fixtures, f, indent=2)

    print(f"Generated parity fixtures with {len(fixtures)} cases in llm-router/training/parity_fixtures.json")

if __name__ == "__main__":
    main()
