"""Reference featurizer for the offline `local` backend.

The Rust implementation in `llm-router/src/routing/local_classifier.rs` mirrors this EXACTLY; parity is
enforced by golden vectors stored in the exported model JSON and checked in a Rust unit test.

Features for one text segment (prefix `q` for the query, `x` for the context):
  * word unigrams            `{p}w:{tok}`
  * word bigrams             `{p}b:{tok1} {tok2}`
  * char 3-5 grams per token `{p}c:{gram}`  (grams of " tok ", token length >= 2)
Weights: (1 + ln tf) * idf, context segment scaled by CONTEXT_SCALE, then L2-normalised over the whole
vector. Six dense shape features are appended after normalisation (see `shape`).
"""

import math
import re
from collections import Counter

CONTEXT_SCALE = 0.5
_TOKEN = re.compile(r"\w+")
_CODEISH = re.compile(r"[{};]|=>|->|::|\bdef\b|\bfn\b")
_DIGIT = re.compile(r"\d")


def tokens(text: str) -> list[str]:
    return _TOKEN.findall(text.lower())


def segment_features(text: str, prefix: str) -> Counter:
    toks = tokens(text)
    out: Counter = Counter()
    for t in toks:
        out[f"{prefix}w:{t}"] += 1
    for a, b in zip(toks, toks[1:]):
        out[f"{prefix}b:{a} {b}"] += 1
    for t in toks:
        if len(t) < 2:
            continue
        padded = f" {t} "
        for n in (3, 4, 5):
            for i in range(len(padded) - n + 1):
                out[f"{prefix}c:{padded[i:i + n]}"] += 1
    return out


def raw_features(query: str, context: str) -> Counter:
    feats = segment_features(query, "q")
    feats.update(segment_features(context, "x"))
    return feats


def shape(query: str, context: str) -> list[float]:
    qw, cw = len(query.split()), len(context.split())
    return [
        math.log1p(qw) / 5.0,
        math.log1p(cw) / 6.0,
        1.0 if ("```" in context or "```" in query) else 0.0,
        1.0 if _CODEISH.search(context + query) else 0.0,
        1.0 if query.strip().endswith("?") else 0.0,
        1.0 if _DIGIT.search(query) else 0.0,
    ]


def weigh(feats: Counter, vocab: dict, idf: list[float]) -> list[tuple[int, float]]:
    """Sparse (index, value) pairs, L2-normalised. Unknown features are dropped."""
    items = []
    for f, tf in feats.items():
        i = vocab.get(f)
        if i is None:
            continue
        v = (1.0 + math.log(tf)) * idf[i]
        if f[0] == "x":
            v *= CONTEXT_SCALE
        items.append((i, v))
    norm = math.sqrt(sum(v * v for _, v in items))
    if norm > 0:
        items = [(i, v / norm) for i, v in items]
    return sorted(items)
