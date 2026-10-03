# Implementation checkpoint

Branch `feat/jev-cache-aware-classifier` contains the P2 work. The local key remains in the ignored root `.env` file.

## Work sequence

1. Trace the classifier, safe routing boundaries, usage records, and provider pricing.
2. Choose the typed classifier interface and compare integration options.
3. Implement the classifier, configuration, routing, and evaluation runner under one code owner.
4. Verify offline fallback, timeout, invalid replies, context, and sticky routing.
5. Run the public smoke set and a separate labelled validation set.
6. Review the diff and document results and limits.

## Throughput checkpoint

- Blocking first steps. Read the P2 brief and verify the JEV Decisions API before code changes.
- Independent workstreams. One worker owns code. The lead owns labelled data, review, and live checks. API research is complete.
- Shared mutable state. Only the code owner changes routing, configuration, handlers, and the evaluation runner. Preserve the setup-generated Cargo.lock changes.
- Smallest safe decomposition. Keep the classifier result in one typed record. Use the same fallback wrapper for the router and evaluation runner.

## Required evidence

- Regex remains the default. Hosted classification requires explicit configuration.
- Latest query and bounded relevant text reach JEV. The answering model keeps its original request.
- Pinned and continue requests do not call JEV or switch models.
- Classification errors, invalid replies, and timeout use regex. Low confidence keeps the current or configured answer model.
- Cache-aware switching uses estimated costs and observed usage. Unknown evidence keeps the current model. Estimates do not prove savings.
- Report accuracy, complexity error, confidence, latency, fallbacks, and available API usage cost. The public smoke set does not prove improvement.

## Worker setting

The user requires GPT-6.1 with low reasoning for delegated implementation and review.
