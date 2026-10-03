# Request Classifier — Architecture Guide

## Overview

The request classifier is a pluggable system that categorizes incoming LLM
requests by type, estimates their complexity, and reports confidence. The
router uses these signals for cost-aware tier selection: simple factual
lookups go to cheap models, complex code generation goes to capable ones.

## Design Principles

### 1. Trait-based pluggability

All backends implement `RequestClassifier`:

```rust
#[async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &'static str { ... }
    fn uses_context(&self) -> bool { false }
    async fn classify(&self, input: &ClassifyInput<'_>)
        -> Result<Classification, ClassifyError>;
}
```

New backends (local models, hosted APIs) plug in without touching the router.

### 2. Fail-safe defaults

- The **regex baseline** is the default. It never fails, needs no network,
  and has no model to load.
- Any backend error (load failure, inference error, timeout) triggers a
  **regex fallback**. Routing continues; the fallback is counted.
- Unknown backend names in config fall back to regex. A typo can't break prod.

### 3. Determinism

All local backends are deterministic: identical inputs produce identical
outputs. This is verified by tests that classify the same input twice and
assert equality. Determinism enables:
- Reproducible evals
- Cacheable decisions
- Debuggable routing

### 4. Honest confidence

Backends report calibrated confidence (0.0–1.0). Low confidence is a signal,
not an error — the router may fall back or request clarification. The
`confidence_from_margin` helper ensures consistent semantics across backends.

## Module Layout

```
llm-router/src/routing/
├── classifier.rs           # Trait, types, regex + heuristic backends, factory
├── classifier_tfidf.rs     # TF-IDF statistical backend
├── classifier_ensemble.rs  # Weighted voting ensemble
└── mod.rs                  # Re-exports
```

## Data Flow

```
                    ┌─────────────────┐
                    │  GatewayConfig  │
                    │ classifier_     │
                    │ backend: "regex"│
                    └────────┬────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │ classifier_from │
                    │ _config()       │
                    └────────┬────────┘
                             │ Arc<dyn RequestClassifier>
                             ▼
┌──────────┐  classify()  ┌──────────────────┐
│  Router  │ ──────────►  │ Backend          │
│          │              │  - regex         │
│          │  ┌────────── │  - heuristic     │
│          │  │ on error  │  - tfidf         │
└──────────┘  ▼           │  - ensemble      │
         ┌────────┐       └──────────────────┘
         │ Regex  │
         │fallback│
         └────────┘
```

## Backend Selection

Configure via environment:

```bash
# Default: regex baseline (safe, no surprises)
CLASSIFIER_BACKEND=regex

# Opt into smarter backends
CLASSIFIER_BACKEND=heuristic   # pattern scoring + context
CLASSIFIER_BACKEND=tfidf        # statistical TF-IDF
CLASSIFIER_BACKEND=ensemble     # weighted vote (best accuracy)

# Timeout for backend calls (ms)
CLASSIFIER_TIMEOUT_MS=100
```

## Adding a New Backend

1. Create `classifier_mybackend.rs` in `routing/`
2. Implement `RequestClassifier`
3. Add a match arm in `classifier_from_backend`
4. Declare the module in `mod.rs` and re-export
5. Document in `CLASSIFIER_BACKENDS.md`
6. Add tests (determinism, accuracy on sample cases, edge cases)

Example:

```rust
use super::classifier::{ClassifyError, ClassifyInput, Classification, RequestClassifier};

pub struct MyBackend;

#[async_trait::async_trait]
impl RequestClassifier for MyBackend {
    fn name(&self) -> &'static str { "mybackend" }
    fn description(&self) -> &'static str { "Does X using Y." }
    fn uses_context(&self) -> bool { true }

    async fn classify(&self, input: &ClassifyInput<'_>)
        -> Result<Classification, ClassifyError>
    {
        // ... implementation ...
        // Never panic. Return Classification::unknown() if unsure.
        Ok(Classification::new(
            RequestType::General,
            2,
            0.5,
            self.name(),
        ))
    }
}
```

## Testing Strategy

- **Unit tests**: each backend has tests for known cases, edge cases (empty
  input, huge input), and determinism.
- **Integration**: the eval example runs all backends on the public sample.
- **Property**: confidence is always in 0.0..=1.0, complexity in 1..=5
  (enforced by `Classification::new`).
- **Regression**: the public sample scores are recorded; CI fails if a
  backend regresses below its recorded accuracy.

## Performance

All local backends classify in microseconds (p50 ~25µs, p95 ~100µs). The
TF-IDF model trains once at first use (~10ms for the embedded corpus) and
is then cached in a `LazyLock`. Memory footprint is under 1MB.

## Security

- Backends never log query content (only IDs and metadata).
- No network access for local backends (verified by design — no HTTP clients).
- Input size is not bounded by the trait, but backends handle huge inputs
  gracefully (tokenization is linear, regexes are non-backtracking).
- The `ClassifyError` never includes query text.

## Future Work

- Hosted backends (API-based classifiers) via the same trait
- Online learning from router feedback
- Multi-label classification for ambiguous queries
- Per-tenant backend selection
