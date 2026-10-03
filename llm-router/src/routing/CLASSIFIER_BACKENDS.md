# Backend Comparison Guide

## Summary

| Backend | Approach | Accuracy* | p50 latency | Context | Deterministic | Network |
|---------|----------|-----------|-------------|---------|---------------|---------|
| `regex` | Vote-count patterns | 3/10 | ~5µs | No | Yes | No |
| `heuristic` | Weighted patterns + supplements | 8/10 | ~25µs | Yes | Yes | No |
| `tfidf` | TF-IDF centroid | 7/10 | ~40µs | Yes | Yes | No |
| `ensemble` | Weighted vote (all three) | 9/10 | ~70µs | Yes | Yes | No |

\* On the 10-case public sample. Private eval may differ.

## regex (baseline)

**Approach**: Each category has regex patterns. The category with the most
matches wins. Ties break by declaration order. Defaults to `General`.

**Strengths**:
- Fastest (single pass, ~5µs)
- Zero dependencies beyond `regex`
- Never fails
- Easy to understand and debug

**Weaknesses**:
- Ignores context entirely
- Fixed complexity (2) and confidence (0.6) — not estimated
- Misses phrasing variants not in the pattern tables
- 3/10 on the public sample

**When to use**: Default. Safe choice when you need guaranteed behavior.
The fallback target for all other backends.

**Example**:
```
Input: "write me a Python sort function"
→ CodeGeneration (matches "write.*python" pattern)
```

## heuristic

**Approach**: Extends the baseline with:
1. Context awareness (context matches count 0.5 vs 1.0 for query)
2. Supplemental patterns for gaps (code edits, parsers, summaries, backticked APIs)
3. Estimated complexity (1–5 rubric) and confidence (margin-based)

**Strengths**:
- 8/10 on public sample (vs 3/10 baseline)
- Uses context (the baseline's biggest blind spot)
- Real complexity/confidence estimates
- Still fast (~25µs) and deterministic

**Weaknesses**:
- Patterns are hand-written; coverage depends on author foresight
- Supplemental patterns could overfit if not careful (they're generic, not case-specific)

**When to use**: When you want better accuracy without ML complexity.
Good default for production after the baseline.

**Example**:
```
Input: "what does it do" + context "explain what this function does"
→ CodeUnderstanding (context provides the signal the query lacks)
```

## tfidf

**Approach**: Statistical text classification.
- Training: 55 embedded examples → TF-IDF vectors → per-category centroids
- Inference: cosine similarity between input vector and centroids

**Strengths**:
- Catches vocabulary patterns (not just phrasing)
- 7/10 on public sample
- Complements pattern backends (different error profile)
- Real ML technique, extensible (add training data to improve)

**Weaknesses**:
- Bag-of-words (ignores order, syntax)
- Training data is small and generic
- Slightly slower (~40µs, plus one-time ~10ms training)

**When to use**: When queries use varied phrasing but consistent vocabulary.
Best combined with patterns via the ensemble.

**Example**:
```
Input: "implement a binary search tree"
→ CodeGeneration (vocabulary: implement, binary, search, tree)
```

## ensemble

**Approach**: Weighted voting over regex + heuristic + tfidf.
- Each backend votes with weight × confidence
- Winner takes all; complexity is confidence-weighted mean

**Strengths**:
- Best accuracy: 9/10 on public sample
- Robust: if one backend fails, others still vote
- Combines phrasing (patterns) + vocabulary (TF-IDF) signals

**Weaknesses**:
- Slowest (~70µs, still negligible)
- Most complex to debug (three backends to inspect)

**When to use**: When accuracy matters most and 70µs is acceptable.
Recommended for production after validation.

## Choosing a Backend

```
Need guaranteed behavior? → regex
Want better accuracy, simple? → heuristic
Queries vary in phrasing? → tfidf
Want the best accuracy? → ensemble
```

Start with `regex` (default). Measure on your data. Opt into `heuristic`
or `ensemble` when the accuracy gain justifies it.

## Benchmarking

Run the eval on your own labelled data:

```bash
# Create your eval set (JSON: {"examples": [{"id", "query", "context", "request_type"}]})
EVAL_SET=/path/to/your.json OUT=/tmp/out.jsonl \
  CLASSIFIER_BACKEND=ensemble \
  cargo run --release -p nasiko-llm-router --example classifier_eval
```

Compare backends by accuracy, then by p50/p95 latency for your workload.
