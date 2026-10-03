# P2 Request Classifier — Labeling Guide

This guide defines our training/validation labels for the Nasiko P2 task.
The seven request-type names are the exact names required by the challenge.

## 1. request_type

Label by the user's primary requested action, not by keywords.

- `code_generation`: produce, modify, debug, refactor, or implement executable code/configuration.
  Examples: implement a function; fix a bug; add an endpoint; write SQL; change a Kubernetes manifest.
- `code_understanding`: explain, interpret, trace, or diagnose existing code without primarily asking for a new implementation.
  Examples: explain what a function does; why a line races; walk through an existing algorithm.
- `technical_design`: design or compare an architecture, API, schema, service boundary, deployment, distributed system, or engineering trade-off.
- `analytical_reasoning`: solve a quantitative/logical problem, derive an answer, prove something, calculate, estimate, or reason through constraints where the requested work is primarily analysis rather than system design.
- `writing`: produce or transform human-facing prose such as an email, article, essay, post, letter, announcement, summary, or story. Rewriting supplied prose stays `writing` unless the main task is code.
- `factual_lookup`: request a relatively direct fact or definition, especially where the answer is a known piece of information rather than a multi-step analysis.
- `general`: casual conversation, vague requests, greetings, broad non-technical requests, or queries that do not fit the six task classes.

### Primary-intent rule
When a request has multiple intents, label the intent that dominates the requested work.
Examples:
- "Explain this function and then rewrite it to use async I/O" -> `code_generation`.
- "Design the API and write a sample endpoint" -> `technical_design` if architecture/API design is the dominant deliverable; otherwise `code_generation`.
- "Find the capital of France and draft an email containing it" -> `writing` because the requested deliverable is the email.

## 2. complexity (1–5)

Complexity is an estimate of how much reasoning/work a capable model must perform to complete the request correctly.

1 — Trivial
- Single small operation.
- Little or no ambiguity.
- Usually <= one short artifact or direct answer.
- No meaningful dependency between steps.
- Example: fix one typo, write a one-line regex, define a term.

2 — Simple
- A few related steps or a small artifact.
- Limited state/context.
- Straightforward correctness criteria.
- Example: implement a short CRUD endpoint; explain a 20-line function; draft a routine email.

3 — Moderate
- Several steps, or moderate context, or multiple interacting requirements.
- Requires choosing among straightforward approaches.
- Example: implement a small service with validation and error handling; design a basic database schema; debug a non-trivial function.

4 — Complex
- Multiple interacting components/constraints, substantial context, or non-obvious trade-offs.
- Requires careful reasoning and checking.
- Example: design a distributed service with retries and observability; debug concurrency behavior across modules; derive a multi-step algorithm under constraints.

5 — Very complex
- System-level or research-like problem with many interacting constraints, ambiguity, or high consequences for correctness.
- Requires deep reasoning, decomposition, and trade-off analysis.
- Example: design a multi-region distributed inference platform with SLOs, failure handling, cost controls, and migration strategy.

### Complexity guardrails
- Text length alone does not determine complexity.
- Mentioning advanced technology does not automatically imply complexity 4–5.
- A long request can still be complexity 1–2 if the task is mechanical.
- A short request can be complexity 4–5 if the hidden reasoning is substantial.
- Complexity is independent of request_type.

## 3. context handling

Use both query and context when context materially changes the task or complexity.
Ignore irrelevant padding.

A missing context value is valid and should not force a low-confidence result.

## 4. ambiguous / multi-intent examples

For ambiguous examples, choose the dominant requested deliverable.
Do not label based on a single trigger word.

## 5. split policy

Never place exact duplicates or obvious paraphrase families across train and validation.
Keep validation examples generated from different semantic templates/domains than the corresponding training examples.

## 6. confidence semantics

The classifier's confidence should represent confidence in the final joint decision.
For a local model we will report calibrated probability-derived confidence, not a hand-written constant.

## 7. tier use

Do not treat the example's `tier_hypothesis` as a label.
The challenge explicitly says it is illustrative, not ground truth.
