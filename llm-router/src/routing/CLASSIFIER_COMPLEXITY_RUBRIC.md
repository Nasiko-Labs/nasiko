# Complexity Rubric (1–5)

## Purpose

Complexity estimates how much work a request represents, guiding tier
selection: trivial queries go to cheap models, complex ones to capable models.
The scale is 1–5, where each level is roughly 2× the effort of the previous.

## Rubric

### 1 — Trivial

**Definition**: A single, simple intent. < 30 characters. No technical content.
Answerable in one sentence.

**Examples**:
- "hi"
- "thanks"
- "what time is it"

**Routing**: Cheapest tier. No need for a capable model.

### 2 — Simple

**Definition**: A single clear intent. 30–120 characters. May have light
technical content but no multi-step reasoning.

**Examples**:
- "what is the capital of France?"
- "write a python function to reverse a string"
- "explain what this loop does"

**Routing**: Cheap tier. Standard models handle these well.

### 3 — Moderate

**Definition**: Multi-step or 120–300 characters, OR contains technical terms
(API, database, algorithm, architecture). Requires some reasoning.

**Examples**:
- "Design a rate limiter for our API and explain how token bucket works"
- "Refactor this class to use dependency injection and add unit tests"
- "Compare SQL vs NoSQL for our workload, considering scale and consistency"

**Routing**: Standard tier. Needs a capable model but not the frontier.

### 4 — Complex

**Definition**: Multi-intent (conjunctions, multiple questions), OR contains
code blocks, OR > 300 characters. Requires deep reasoning or synthesis.

**Examples**:
- "Design a distributed cache with write-through and write-behind, and also explain the CAP trade-offs? Also compare Redis vs Memcached?"
- "Implement a parser for this grammar: [code block]. Handle errors gracefully and add tests."

**Routing**: Capable tier. Needs a strong model.

### 5 — Very Complex

**Definition**: > 500 characters OR multi-domain (spans 2+ categories deeply).
Requires expert-level synthesis.

**Examples**:
- A 600-word system design doc request covering architecture, database schema, API design, and deployment strategy.

**Routing**: Frontier tier. Use the best available model.

## How Backends Estimate

### heuristic

1. Start at 2 (simple).
2. +1 if multi-intent (2+ conjunctions/questions).
3. +1 if technical (code blocks, API/database/algorithm mentions, or "step by step"/"detailed"/"comprehensive").
4. +1 if > 300 chars.
5. Return 5 immediately if > 500 chars.
6. Return 1 if < 30 chars and not technical.

### tfidf

Length-based only (the TF-IDF model doesn't analyze structure):
- < 30 chars → 1
- < 120 chars → 2
- < 300 chars → 3
- < 500 chars → 4
- ≥ 500 chars → 5

### regex (baseline)

Always 2. The baseline cannot estimate complexity; this is documented so
evaluators can distinguish "baseline" from "estimated".

### ensemble

Confidence-weighted mean of member backends' estimates, rounded.

## Calibration

The rubric is calibrated so that:
- Most queries are 2–3 (the bulk of real traffic)
- 1 and 5 are rare (tails of the distribution)
- Each level is ~2× the effort of the previous (for cost modeling)

If your workload differs, adjust the thresholds — but document the change.

## Examples by Category

### CodeGeneration
- 1: "hi" (not code, but trivial)
- 2: "write a sort function"
- 3: "implement a REST API with auth and rate limiting"
- 4: "build a parser for this grammar [code] with error recovery"
- 5: "design and implement a distributed database [500+ words]"

### Writing
- 2: "draft an email about the delay"
- 3: "write a blog post comparing our product to competitors"
- 4: "create comprehensive documentation for the API with examples"

## Anti-patterns

- **Don't** use complexity as a proxy for importance. A trivial query can be urgent.
- **Don't** assume longer = more complex. "Summarize this 1000-word doc in one sentence" is long input but simple output.
- **Do** re-calibrate if your tier boundaries change.
