"""Shared feature extraction: frozen bge-small embeddings of query and context + cheap shape features."""

import re

import numpy as np

ENCODER_NAME = "BAAI/bge-small-en-v1.5"
LABELS = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
]

_encoder = None


def encoder():
    global _encoder
    if _encoder is None:
        from fastembed import TextEmbedding

        _encoder = TextEmbedding(ENCODER_NAME)
    return _encoder


def embed(texts):
    vecs = np.array(list(encoder().embed(texts)), dtype=np.float32)
    return vecs / np.maximum(np.linalg.norm(vecs, axis=1, keepdims=True), 1e-9)


def shape_features(query: str, context: str) -> list[float]:
    qw, cw = len(query.split()), len(context.split())
    return [
        np.log1p(qw) / 5,
        np.log1p(cw) / 6,
        float("```" in context or "```" in query),
        float(bool(re.search(r"[{};]|=>|->|::|\bdef \b|\bfn \b", context + query))),
        float(query.strip().endswith("?")),
        float(bool(re.search(r"\d", query))),
    ]


def featurize(queries: list[str], contexts: list[str]) -> np.ndarray:
    q = embed(queries)
    # Empty context embeds to zeros so "no context" is a distinct, stable signal.
    has_ctx = np.array([bool(c.strip()) for c in contexts])
    c = np.zeros_like(q)
    if has_ctx.any():
        c[has_ctx] = embed([x for x, h in zip(contexts, has_ctx) if h])
    shape = np.array([shape_features(a, b) for a, b in zip(queries, contexts)], dtype=np.float32)
    return np.hstack([q, 0.5 * c, shape])
