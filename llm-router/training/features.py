"""
P2 Feature Extractor — Python reproduction of Rust feature extraction.

Must match `llm-router/src/routing/request_classifier.rs` and `patterns.rs` exactly.
"""

import math
import re

OFFSET_BASIS = 0xcbf29ce484222325
PRIME = 0x00000100000001b3
MASK64 = 0xffffffffffffffff
NUM_BUCKETS = 65536

REQUEST_TYPE_CLASSES = [
    "code_generation",
    "code_understanding",
    "technical_design",
    "analytical_reasoning",
    "writing",
    "factual_lookup",
    "general",
]

COMPLEXITY_CLASSES = [1, 2, 3, 4, 5]

CONSTRAINT_WORDS = [
    "must", "ensure", "require", "requires", "constraint", "constraints",
    "necessary", "mandatory", "need to", "needs to", "have to", "has to",
    "should", "ought",
]

SEQUENTIAL_WORDS = [
    "step", "steps", "first", "then", "next", "finally", "lastly",
    "procedure", "process", "workflow", "pipeline", "sequence",
]

# Patterns identical to `llm-router/src/routing/patterns.rs`
CATEGORY_PATTERNS = [
    (
        "code_generation",
        [
            re.compile(r"(?i)\b(write|implement|create|build|generate)\b.{0,40}\b(function|script|code|class|program|api|endpoint|method|module)\b"),
            re.compile(r"(?i)\bwrite (a|an|the|me)\b.*\b(python|javascript|typescript|rust|golang|go|java|c\+\+|sql)\b"),
            re.compile(r"(?i)\bfix (this|the) bug\b"),
            re.compile(r"(?i)\brefactor\b"),
            re.compile(r"(?i)\badd error handling\b"),
        ],
    ),
    (
        "code_understanding",
        [
            re.compile(r"(?i)\bexplain (what|how|why)\b"),
            re.compile(r"(?i)\bwhat does (this|that|the) (function|code|script|class) do\b"),
            re.compile(r"(?i)\bhow does (this|that|the) (function|code|script|class) work\b"),
            re.compile(r"(?i)\bwalk me through (this|that) code\b"),
            re.compile(r"(?i)\bwhat is this code doing\b"),
        ],
    ),
    (
        "technical_design",
        [
            re.compile(r"(?i)\bhow should i design\b"),
            re.compile(r"(?i)\b(api|system|database|schema) design\b"),
            re.compile(r"(?i)\barchitecture\b"),
            re.compile(r"(?i)\bdesign (a|an|the) (system|api|service|schema|database)\b"),
            re.compile(r"(?i)\btrade-?offs?\b"),
        ],
    ),
    (
        "analytical_reasoning",
        [
            re.compile(r"(?i)\bcalculate\b"),
            re.compile(r"(?i)\bprobability\b"),
            re.compile(r"(?i)\bsolve\b"),
            re.compile(r"(?i)\bprove\b"),
            re.compile(r"(?i)\bproof\b"),
            re.compile(r"(?i)\bhow many\b"),
            re.compile(r"(?i)\bwhat'?s the (sum|product|average|result)\b"),
            re.compile(r"[0-9]+\s*[+\-*/]\s*[0-9]+"),
        ],
    ),
    (
        "writing",
        [
            re.compile(r"(?i)\bdraft\b"),
            re.compile(r"(?i)\bwrite (an?|the)\b.*\b(email|blog|article|essay|post|letter|story|poem)\b"),
            re.compile(r"(?i)\bcompose\b"),
            re.compile(r"(?i)\brewrite (this|that|the)\b"),
            re.compile(r"(?i)\bmake this sound\b"),
        ],
    ),
    (
        "factual_lookup",
        [
            re.compile(r"(?i)\bwhat is (the )?capital of\b"),
            re.compile(r"(?i)^\s*(who|what|when|where) (is|was|are|were)\b"),
            re.compile(r"(?i)\bdefine\b"),
            re.compile(r"(?i)\bhow many\b.*\b(are there|exist)\b"),
        ],
    ),
]


def classify_request_type(text: str) -> str:
    best = "general"
    best_score = 0
    for rt, pats in CATEGORY_PATTERNS:
        score = sum(1 for p in pats if p.search(text))
        if score > best_score:
            best_score = score
            best = rt
    return best


def regex_vote_features(query: str) -> list[float]:
    rt = classify_request_type(query)
    idx = REQUEST_TYPE_CLASSES.index(rt) if rt in REQUEST_TYPE_CLASSES else 6
    feats = [0.0] * 7
    feats[idx] = 1.0
    return feats


def fnv1a(b: bytes) -> int:
    h = OFFSET_BASIS
    for byte in b:
        h = ((h ^ byte) * PRIME) & MASK64
    return h


def hash_to_bucket(gram: str) -> tuple[int, float]:
    h = fnv1a(gram.encode("utf-8"))
    bucket = h % NUM_BUCKETS
    sign_bit = fnv1a(f"sign:{gram}".encode("utf-8")) & 1
    sign = 1.0 if sign_bit == 0 else -1.0
    return bucket, sign


def word_tokens(text: str) -> list[str]:
    tokens = []
    current = []
    for c in text.lower():
        if c.isalnum():
            current.append(c)
        else:
            if current:
                tokens.append("".join(current))
                current = []
    if current:
        tokens.append("".join(current))
    return tokens


def word_ngrams(tokens: list[str], max_n: int = 2) -> list[str]:
    grams = []
    for n in range(1, max_n + 1):
        if n > len(tokens):
            break
        for i in range(len(tokens) - n + 1):
            grams.append(" ".join(tokens[i : i + n]))
    return grams


def char_ngrams(text: str, min_n: int = 3, max_n: int = 5) -> list[str]:
    chars = list(text.lower())
    grams = []
    for n in range(min_n, max_n + 1):
        if n > len(chars):
            break
        for i in range(len(chars) - n + 1):
            grams.append("".join(chars[i : i + n]))
    return grams


def p2_dense_features(query: str, context: str | None = None) -> list[float]:
    q_lower = query.lower()
    tokens = word_tokens(query)
    char_count = float(len(query))

    # Sentence count
    raw_sentences = re.split(r"[.!?]", query)
    sentence_count = float(max(1, len([s for s in raw_sentences if s.strip()])))

    has_question_mark = 1.0 if "?" in query else 0.0
    has_code_fence = 1.0 if ("```" in query or "~~~" in query) else 0.0
    has_url = 1.0 if any(u in q_lower for u in ["http://", "https://", "www."]) else 0.0

    list_items = 0
    for line in query.splitlines():
        t = line.strip()
        if not t:
            continue
        if t.startswith("-") or t.startswith("*"):
            list_items += 1
        elif len(t) > 0 and t[0].isdigit() and "." in t:
            list_items += 1

    constraint_count = float(sum(1 for w in CONSTRAINT_WORDS if w in q_lower))
    sequential_count = float(sum(1 for w in SEQUENTIAL_WORDS if w in q_lower))
    context_len = float(len(context)) if context else 0.0

    features = [0.0] * 10
    features[0] = math.log1p(sentence_count)
    features[1] = has_question_mark
    features[2] = has_code_fence
    features[3] = has_url
    features[4] = math.log1p(float(list_items))
    features[5] = math.log1p(constraint_count)
    features[6] = math.log1p(sequential_count)
    features[7] = math.log1p(context_len)
    features[8] = math.log1p(char_count)
    features[9] = math.log1p(float(len(tokens)))
    return features


def p2_hashed_features(query: str, context: str | None = None) -> dict[int, float]:
    ctx_prefix = (context or "")[:300]
    if ctx_prefix:
        combined = f"{ctx_prefix} {query}"
    else:
        combined = query

    tokens = word_tokens(combined)
    grams = word_ngrams(tokens, 2)
    grams.extend(char_ngrams(query, 3, 5))

    norm = max(1.0, math.sqrt(len(grams)))
    by_bucket: dict[int, float] = {}
    for gram in grams:
        bucket, sign = hash_to_bucket(gram)
        by_bucket[bucket] = by_bucket.get(bucket, 0.0) + (sign / norm)
    return by_bucket


def extract_full_features(query: str, context: str | None = None) -> tuple[dict[int, float], list[float]]:
    """Returns (hashed_dict, dense_tail) where dense_tail is length 17 (10 dense + 7 regex)."""
    hashed = p2_hashed_features(query, context)
    dense = p2_dense_features(query, context)
    regex_votes = regex_vote_features(query)
    dense_tail = dense + regex_votes
    return hashed, dense_tail
