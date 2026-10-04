# TASK: Implement Nasiko P2 — Intelligent Request Classifier for Cost-Aware Routing

You are working inside an **already-cloned and installed Nasiko OSS repository**.

Do NOT recreate the repository, reinstall it, initialize a new project, or disturb the existing environment.

Your task is to implement the **P2 — Request classifier for cost-aware routing** challenge described below.

The implementation must be **merge-ready, scoped, deterministic, testable, demonstrable, and compatible with the existing Nasiko architecture**.

The most important principle:

> **Do not redesign Nasiko. Inspect the existing architecture first and integrate the classifier into the correct existing routing flow.**

The classifier is the core deliverable.

It must return a structured JSON-compatible classification/requirement profile. A downstream routing/model-selection component will consume that profile later. **Do NOT implement a new model-selection/capability-benchmark framework in this task.**

---

# 1. FIRST: UNDERSTAND THE EXISTING REPOSITORY

Before modifying anything:

1. Inspect the repository structure.
2. Locate:

   * `llm-router/`
   * the existing routing implementation
   * the existing request-type classifier
   * the file containing the existing regex expressions/rules. There is expected to be a file/component named something like `classifiers`; if the exact location/name differs, locate the actual implementation through code search.
   * `classify_request_type`
   * `RequestType`
   * routing boundaries such as `cold_start`, `switch`, and `continue`
   * `llm-router/examples/classifier_eval.rs`
   * existing configuration handling in the binary, especially `config.rs`
   * existing tests related to routing/classification
3. Read the relevant source files before making architectural decisions.
4. Determine how the current regex classifier flows into the router.
5. Determine the existing module conventions, error handling, async conventions, configuration patterns, logging patterns, and test patterns.

Do not create duplicate components if equivalent infrastructure already exists.

Do not assume file names from this prompt are exact. If the regex classifier is not in a file literally called `classifiers`, find the real implementation and integrate there.

Use the repository's existing architecture wherever possible.

---

# 2. LANGUAGE / TECHNOLOGY

The implementation language is:

**Rust**

Use the existing Rust edition, workspace configuration, dependency versions, and project conventions.

Do not introduce another implementation language for the core classifier.

Do not introduce Python into the production path.

Do not create a separate Node/TypeScript service.

The classifier must live inside the existing `llm-router` Rust crate/module structure.

---

# 3. OFFICIAL P2 REQUIREMENT

The existing Nasiko router currently uses regex/keyword classification.

The problem is that regex can identify broad request type but does not adequately capture:

* request complexity
* contextual complexity
* conversation dependency
* codebase size
* change impact
* debugging difficulty
* reasoning requirements
* tool/agentic requirements
* other structural characteristics of the request

The new classifier should therefore understand the **requirements of the request**, rather than relying purely on keywords.

The official P2 contract requires an interface equivalent to:

```rust
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,   // 1–5
    pub confidence: f32,  // 0–1
}

#[async_trait::async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;

    async fn classify(
        &self,
        input: &ClassifyInput<'_>
    ) -> Result<Classification, ClassifyError>;
}
```

Equivalent signatures are acceptable if they fit the repository's existing conventions.

The existing regex classifier must remain available as the default/fallback behavior.

---

# 4. IMPORTANT SCOPE DECISION

Do NOT implement the following as part of this task:

* a model capability benchmark database
* benchmarking every available LLM
* a new model-selection engine
* agent-selection redesign
* orchestrator redesign
* flow-branch decision engine
* TokenOps dashboard
* production-grade security/malicious-input platform
* a complex reinforcement-learning/bandit system
* automatic model capability scoring
* an entirely new router architecture

The classifier should produce a **request requirement profile**.

A future/downstream component will use that profile to select a model/tier.

The architecture must make that possible without implementing the downstream model-selection system now.

---

# 5. CORE ARCHITECTURE

Implement this conceptual flow:

```text
                         USER REQUEST
                              |
                              v
                  +-----------------------+
                  | Existing Router Input |
                  | query + context       |
                  +-----------+-----------+
                              |
                              v
                  +-----------------------+
                  | Intelligent ML        |
                  | Request Classifier    |
                  +-----------+-----------+
                              |
             +----------------+----------------+
             |                                 |
             v                                 v
      Semantic Analysis                 Existing/System
      / ML-derived fields               context/state
             |                                 |
             +----------------+----------------+
                              |
                              v
                 REQUEST REQUIREMENT PROFILE
                              |
                              v
                     JSON CLASSIFICATION
                              |
                              v
                  Downstream Router / Tier
                     Selection (future)
```

The classifier itself stops at the structured classification/profile.

Do NOT hard-code a specific model choice such as:

```text
use GPT-X
use model Y
use tier Z
```

inside the classifier.

---

# 6. VERY IMPORTANT: DO NOT MAKE REGEX THE FIRST HARD GATE

There is an existing regex classifier.

Do NOT implement:

```text
Regex
  ↓
code_generation
  ↓
ML only analyzes code_generation
```

That would anchor the ML model to potentially incorrect regex output.

Instead:

```text
                 REQUEST + CONTEXT
                        |
              +---------+---------+
              |                   |
              v                   v
        Existing Regex        ML Classifier
              |                   |
              |             Requirement Profile
              |                   |
              +---------+---------+
                        |
                        v
                 Final Classification
```

The regex implementation remains:

1. the default implementation when the experimental classifier is disabled,
2. a fallback when ML loading/inference/network/timeout fails,
3. a baseline for evaluation,
4. existing behavior that must not regress.

The ML classifier must be capable of correcting cases where regex would have classified the request incorrectly.

Do not use the regex result as a hard constraint on ML.

---

# 7. REQUIRED REQUEST TYPES

Preserve the existing `RequestType` values:

```text
code_generation
code_understanding
technical_design
analytical_reasoning
writing
factual_lookup
general
```

Do not invent incompatible request types.

Use the existing `RequestType` enum if it already exists.

---

# 8. REQUIREMENT PROFILE

The classifier should produce the official fields plus a richer structured profile.

The conceptual JSON should be:

```json
{
  "request_type": "code_generation",
  "complexity": 4,
  "confidence": 0.91,

  "profile": {
    "reasoning_depth": 3,
    "coding_complexity": 4,
    "debugging_difficulty": 2,
    "tool_orchestration": 3,
    "conversation_dependency": 5,
    "change_impact": 5
  },

  "context": {
    "codebase_size": 4,
    "context_size": 4,
    "cache_affinity": true
  }
}
```

The exact serialized shape should follow existing repository conventions and the official evaluator's required fields.

The mandatory fields are:

```text
request_type
complexity
confidence
```

Additional profile information may be included if compatible with the evaluator.

If the evaluator's JSON schema is strict, preserve the evaluator output contract and expose the richer profile through the internal classifier/API without breaking evaluation.

Do NOT blindly change `classifier_eval.rs` to emit a nonstandard format that the official scorer cannot consume.

Inspect the existing evaluator before finalizing the serialization.

---

# 9. ML-DERIVED DIMENSIONS

The ML classifier should estimate these dimensions:

## 9.1 Reasoning Depth

How much conceptual/analytical reasoning is required?

Examples:

Low:

* rename a variable
* fix a typo
* add a simple condition

Medium:

* explain why an implementation behaves a certain way
* modify an algorithm with limited tradeoffs

High:

* architectural tradeoffs
* distributed-system design
* algorithm selection
* complex conceptual synthesis

Range:

```text
1–5
```

---

## 9.2 Coding Complexity

How difficult is the requested code change?

Consider:

* amount of code required
* algorithmic complexity
* number of interacting components
* refactoring difficulty
* dependency relationships
* correctness sensitivity

Important:

**Code size is NOT equivalent to coding complexity.**

---

## 9.3 Debugging Difficulty

Estimate how difficult it is to diagnose the problem.

Examples:

Low:

* obvious syntax error
* typo
* straightforward compiler error

Medium:

* isolated runtime bug

High:

* ambiguous production behavior
* race conditions
* distributed-system failure
* interaction between multiple components
* insufficient evidence

Range:

```text
1–5
```

---

## 9.4 Tool / Agentic Complexity

Estimate how much tool orchestration may be required.

Consider:

* multiple files
* repository inspection
* external APIs
* terminal commands
* IDE operations
* database access
* multi-step investigation
* iterative tool usage

Range:

```text
1–5
```

This is a classifier signal only.

Do not implement tool execution here.

---

## 9.5 Conversation Dependency

Determine how strongly the current request depends on previous conversation turns.

Example:

```text
"Fix this function."
```

with the function provided directly:

low dependency.

But:

```text
"Now change the retry logic we discussed earlier."
```

after several turns containing a large codebase:

high dependency.

Range:

```text
1–5
```

The classifier should consider the supplied `context`.

Do not assume the query can always be understood independently.

---

## 9.6 Change Impact

This is an important custom signal.

Do NOT equate:

```text
number of changed lines
```

with:

```text
impact
```

The classifier should estimate semantic/system impact.

Example:

```text
"Change this local variable name."

coding complexity: low
change impact: low
```

versus:

```text
"Change this one line in authentication middleware."
```

could be:

```text
coding complexity: low
change impact: high
```

Likewise:

```text
"Build a large UI settings page with 15 components."
```

could have:

```text
coding complexity: high
change impact: medium
```

depending on the context.

This distinction is intentional.

---

# 10. SYSTEM/CONTEXT-DERIVED DIMENSIONS

Do not force the ML model to hallucinate facts that the system already knows.

Where the router has actual state, use actual state.

Examples:

```text
codebase_size
context_size
cache_affinity
cached_model_family
conversation_position
```

These are different from semantic predictions.

For example:

```text
codebase_size = 4
```

should preferably be calculated from available repository/context metadata rather than guessed by the model.

Similarly:

```text
cache_affinity = true
```

should be derived from actual cache state if that information exists.

---

# 11. CACHE / CONVERSATION HISTORY

Implement the architecture so cache/conversation information can be represented without making the classifier responsible for cache management.

Important scenario:

Turn 1:

```text
User provides a large codebase/module.
```

Turn 2:

```text
"Now fix the retry logic."
```

The second query is short but may have very high:

```text
conversation_dependency
```

and potentially high:

```text
cache_affinity
```

because the request depends on the previously supplied code.

The router should therefore not confuse:

```text
short query
```

with:

```text
simple request
```

If the existing router exposes cache information, use it.

If it does not, create a clean abstraction/interface for future cache state rather than inventing fake cache state.

Do not implement an entirely new cache subsystem.

---

# 12. CODEBASE SIZE

Where actual codebase/repository information is available, expose it as context.

The classifier should distinguish between:

```text
small isolated snippet
```

and:

```text
large repository/module context
```

Possible normalized representation:

```text
1 = tiny / isolated
2 = small
3 = medium
4 = large
5 = very large
```

Do not fabricate repository metrics.

If the current classifier API only receives query + optional context, derive this from the available context representation or keep the field optional/defaulted.

Document how it is calculated.

---

# 13. COMPLEXITY

The official classifier requires:

```text
complexity: 1–5
```

This should be the high-level complexity output.

The richer profile explains its components.

For example:

```json
{
  "complexity": 5,
  "profile": {
    "reasoning_depth": 4,
    "coding_complexity": 3,
    "debugging_difficulty": 5,
    "tool_orchestration": 4,
    "conversation_dependency": 5,
    "change_impact": 5
  }
}
```

Do not simply calculate complexity from the number of words.

Do not use token count as a proxy for complexity.

---

# 14. CONFIDENCE

Return:

```text
confidence ∈ [0,1]
```

Confidence must represent the classifier's confidence in the classification.

Do not always return:

```text
0.99
```

Do not hard-code confidence values for the ML backend.

For ambiguous requests, confidence should decrease.

Examples:

```text
clear request → high confidence
ambiguous/multi-intent request → lower confidence
out-of-distribution request → lower confidence
```

If the underlying ML model cannot provide calibrated confidence, implement a documented confidence estimation strategy appropriate to the chosen model.

Document the semantics.

---

# 15. CHOOSE THE ML APPROACH PRAGMATICALLY

Do not over-engineer.

The P2 challenge explicitly allows:

* embeddings + small classifier
* distilled transformer
* compact decision model
* local model
* hosted model

Choose the approach that is:

1. reliable,
2. deterministic,
3. CPU-runnable within the evaluator constraints,
4. simple enough to maintain,
5. fast enough to justify its routing value,
6. easy to demonstrate.

The classifier must load the model once rather than loading it per request.

The evaluation harness expects model loading to happen once before the classification loop.

Per-call latency should exclude one-time load.

---

# 16. TRAINING DATA

The P2 requirements expect labelled train/validation data with documented labelling criteria.

If the repository already contains an appropriate dataset or infrastructure, inspect and use it.

If training data must be generated locally, create a small, documented dataset suitable for the chosen classifier.

Document:

* labeling criteria
* how each dimension is defined
* how complexity is assigned
* how ambiguous cases are handled
* how multi-intent requests are labeled
* how near-duplicates are avoided between training and validation

Do not hard-code the public evaluation answers.

Do not create rules that identify individual public test IDs.

The private evaluator contains unseen examples and deliberately includes:

* ambiguous queries
* multi-intent queries
* paraphrases
* out-of-distribution queries
* noisy/padded queries
* near-miss cases where regex fails

The implementation must generalize.

---

# 17. DETERMINISM

Identical inputs and identical state must produce identical outputs.

The official evaluator runs the implementation twice and checks determinism.

Therefore:

* use deterministic inference where possible
* use temperature 0 for hosted model backends
* fix seeds where applicable
* avoid random sampling unless explicitly controlled
* if sampling is unavoidable, expose a seed and document variance

Do not use timestamps, random IDs, or nondeterministic ordering in classifier output.

---

# 18. REGEX FALLBACK

The existing regex classifier must remain intact as the fallback.

Fallback must occur when:

* ML model fails to load
* ML inference fails
* hosted backend fails
* network failure
* timeout
* invalid classifier output
* malformed response

On fallback:

```text
regex classification
+
documented fallback complexity/confidence
```

must be returned.

Count fallback occurrences if the existing architecture has metrics/instrumentation for this.

Do not crash the router merely because the experimental classifier fails.

---

# 19. CONFIGURATION

Backend selection must be configurable.

Follow the existing configuration architecture.

The P2 requirement allows concepts such as:

```text
CLASSIFIER_BACKEND=regex|local|hosted
CLASSIFIER_MODEL_PATH=...
CLASSIFIER_ENDPOINT=...
```

Use the repository's existing config conventions rather than inventing an unrelated configuration framework.

Important:

Configuration belongs in the binary/configuration layer.

Do not read environment variables directly from the core library/trait implementation if the repository architecture separates configuration from library code.

The core classifier should receive configured values/dependencies.

---

# 20. DEFAULT BEHAVIOR

Existing behavior must remain the default.

That means:

```text
default Nasiko
      ↓
existing regex classifier
```

must continue to work.

The ML classifier should be:

```text
experimental / opt-in
```

unless the repository's existing challenge branch already has a different mechanism.

Do not silently replace regex as the production default.

---

# 21. ROUTER INTEGRATION

Integrate the classifier at the existing safe routing boundaries:

```text
cold_start
switch
```

Do not reclassify on every `continue` step of a tool loop.

Selected tier/model routing should remain sticky during:

```text
continue
```

unless existing architecture explicitly dictates otherwise.

Do not move classification into the orchestrator.

The classifier should sit at the routing layer.

---

# 22. TRAIT / ARCHITECTURE

Prefer something equivalent to:

```rust
pub struct ClassifyInput<'a> {
    pub query: &'a str,
    pub context: Option<&'a str>,
}

pub struct Classification {
    pub request_type: RequestType,
    pub complexity: u8,
    pub confidence: f32,

    pub profile: RequirementProfile,
}

pub struct RequirementProfile {
    pub reasoning_depth: u8,
    pub coding_complexity: u8,
    pub debugging_difficulty: u8,
    pub tool_orchestration: u8,
    pub conversation_dependency: u8,
    pub change_impact: u8,

    // Context/system information where available.
    pub codebase_size: Option<u8>,
    pub context_size: Option<u8>,
    pub cache_affinity: Option<bool>,
}

#[async_trait::async_trait]
pub trait RequestClassifier: Send + Sync {
    fn name(&self) -> &str;

    async fn classify(
        &self,
        input: &ClassifyInput<'_>
    ) -> Result<Classification, ClassifyError>;
}
```

Adapt this to the existing repository rather than blindly copying it.

If adding fields to `Classification` would break existing consumers, introduce a compatible internal/profile representation while preserving the required public/evaluator contract.

---

# 23. JSON OUTPUT

The system must be demonstrable by passing a request and receiving JSON.

Example:

Input:

```text
Fix the authentication middleware so that expired OAuth tokens are refreshed correctly. The relevant implementation spans several files and the previous conversation contains the current middleware implementation.
```

Expected conceptual output:

```json
{
  "request_type": "code_generation",
  "complexity": 5,
  "confidence": 0.91,
  "profile": {
    "reasoning_depth": 4,
    "coding_complexity": 4,
    "debugging_difficulty": 4,
    "tool_orchestration": 3,
    "conversation_dependency": 5,
    "change_impact": 5
  },
  "context": {
    "codebase_size": 4,
    "context_size": 4,
    "cache_affinity": true
  }
}
```

The exact values above are illustrative only.

Do NOT hard-code these outputs.

The actual classifier must produce them based on input.

---

# 24. DEMO REQUIREMENT

I need to be able to demonstrate the classifier easily.

Inspect the existing:

```text
llm-router/examples/classifier_eval.rs
```

and extend it rather than creating a competing evaluator.

The official command is:

```bash
EVAL_SET=/tmp/classifier-eval.json \
OUT=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

This command must continue to work.

The example should:

1. read the evaluation set,
2. initialize the classifier once,
3. classify every case,
4. output JSONL,
5. include required fields,
6. optionally expose the richer requirement profile if compatible,
7. report latency,
8. return exit code 0 when evaluation completes successfully.

Do not alter the evaluator in a way that makes the official harness incompatible.

---

# 25. ADD A SIMPLE HUMAN DEMO IF APPROPRIATE

If the repository structure permits it without disrupting the official evaluator, provide a simple demonstration path.

For example:

```bash
cargo run --release -p nasiko-llm-router --example classifier_demo -- \
  "Fix the authentication middleware bug..."
```

or an equivalent existing example mechanism.

The goal is:

```text
INPUT
  ↓
classifier
  ↓
JSON
```

I should be able to run it during the hackathon demo and show the JSON output live.

The demo must not require hard-coded API keys.

If using a hosted backend, follow the repository's existing environment/configuration pattern.

Prefer a local/offline demonstration where practical.

---

# 26. TESTING

Write tests for the classifier and integration.

At minimum test:

### Request type

* code generation
* code understanding
* technical design
* analytical reasoning
* writing
* factual lookup
* general

### Complexity

Test:

* trivial request
* moderate request
* complex request

### Ambiguity

Test:

* ambiguous request
* multi-intent request
* short but context-heavy request

### Context dependency

Test:

```text
small query + no context
```

versus:

```text
small query + large previous context
```

### Change impact

Test:

```text
large but isolated UI feature
```

versus:

```text
small authentication/payment/core-system modification
```

### Fallback

Simulate:

* model-load failure
* inference failure
* timeout
* malformed ML response

and verify regex fallback.

### Determinism

Run identical input twice and verify identical output.

### Sticky routing

Verify classifier is invoked at:

```text
cold_start
switch
```

and not repeatedly during:

```text
continue
```

where existing architecture defines sticky routing.

---

# 27. DO NOT CONFUSE TOKEN COUNT WITH COMPLEXITY

This is a major requirement.

A 2,000-token prompt is not necessarily more complex than a 200-token prompt.

For example:

```text
"Implement a simple 20-component UI page."
```

may have a large output footprint but modest reasoning/impact.

Meanwhile:

```text
"Change one line in the payment authorization flow."
```

may be tiny but high-impact.

The classifier should use semantic/contextual information.

---

# 28. DO NOT CONFUSE CODE SIZE WITH IMPACT

Similarly:

```text
lines changed ≠ impact
```

Impact should consider:

* component criticality
* dependency breadth
* architectural position
* security sensitivity
* shared infrastructure
* downstream effects
* scope of affected behavior

Do not build a simplistic formula such as:

```text
impact = changed_lines / 100
```

---

# 29. DO NOT IMPLEMENT SECURITY AS THE CORE FEATURE

A future/optional extension may classify risky or malicious input.

Do NOT allow this to take over the P2 implementation.

The current priority is:

1. request type
2. complexity
3. confidence
4. multidimensional requirement profile
5. context/conversation characteristics
6. deterministic behavior
7. regex fallback
8. router integration
9. evaluation
10. demo

If there is remaining time, a risk signal may be introduced as an optional extension, but it must not compromise the required classifier.

---

# 30. NO MODEL CAPABILITY MATRIX YET

Do not implement:

```text
GPT-4 capability = ...
Claude capability = ...
Gemini capability = ...
```

or any other hard-coded model capability vector.

Do not create a benchmark database.

Do not implement model scoring yet.

The classifier's responsibility is only:

```text
REQUEST
    ↓
REQUIREMENT PROFILE
    ↓
JSON
```

A later component can consume:

```text
Requirement Profile
        ↓
Benchmark-based Model Selection
```

This separation is intentional.

---

# 31. EVALUATION REQUIREMENTS

The official P2 evaluator measures:

* request-type accuracy against regex baseline
* confidence/calibration
* complexity performance
* p50 latency
* p95 latency
* model load time
* hardware
* cost per decision
* fallback behavior

The classifier must therefore expose enough information for evaluation.

Do not optimize only for the public 10-case smoke test.

The private set contains approximately 200 cases and includes difficult examples.

Do not hard-code public cases.

---

# 32. DOWNSTREAM ROUTING CLAIMS

Do not claim:

> "This saves money."

unless it is actually measured.

The P2 brief explicitly requires downstream evidence—answer quality, model usage, tokens, and cost—before claiming that routing is cheaper or better.

For this implementation, focus on producing a high-quality classifier/profile.

The downstream routing experiment can consume the JSON later.

---

# 33. LATENCY

Classifier latency matters because the classifier itself runs before routing.

Do not create an unnecessarily enormous model.

Measure:

```text
model load time
p50 classification latency
p95 classification latency
```

Per-call latency should exclude one-time model load.

For hosted backends, include network latency.

---

# 34. ERROR HANDLING

Use typed Rust errors.

Do not use:

```rust
unwrap()
```

in production paths where failure is possible.

Gracefully handle:

* missing configuration
* model load failure
* invalid model output
* invalid classification
* timeout
* network failure
* malformed JSON
* missing context
* unexpected input

Fallback to regex where appropriate.

---

# 35. VALIDATION OF CLASSIFIER OUTPUT

Ensure:

```text
complexity ∈ 1..=5
confidence ∈ 0.0..=1.0
```

and all profile dimensions are within their documented range.

If the ML backend returns invalid values:

1. reject the invalid result,
2. trigger fallback if appropriate,
3. do not silently produce corrupted routing data.

---

# 36. DOCUMENTATION

Update/add documentation describing:

1. architecture
2. classifier interface
3. request-type definitions
4. complexity rubric
5. profile dimensions
6. confidence semantics
7. backend configuration
8. fallback behavior
9. deterministic behavior
10. how to run the demo
11. how to run the official evaluator
12. known limitations
13. what is not implemented yet

Include a simple architecture diagram in Markdown.

---

# 37. README / DEMO EXPLANATION

The central demo story should be:

```text
Traditional router:

"What kind of request is this?"

Our classifier:

"What does this request actually require?"
```

Example:

```text
Prompt:
"Change this one line in the authentication middleware."

Traditional keyword classifier:
code_generation

Our classifier:
code_generation
complexity: 4
coding_complexity: 2
reasoning_depth: 3
change_impact: 5
conversation_dependency: ...
```

The important point is that **request size is not the same thing as request difficulty or impact**.

---

# 38. IMPORTANT: INSPECT BEFORE MODIFYING

Before implementing anything, explicitly identify:

```text
1. Existing classifier file
2. Existing RequestType
3. Existing routing entry points
4. Existing classifier_eval.rs
5. Existing config.rs
6. Existing router construction
7. Existing tests
8. Existing error types
9. Existing async runtime
10. Existing dependencies suitable for the classifier
```

Then make the smallest coherent architectural changes.

Do not duplicate an existing abstraction.

---

# 39. IMPLEMENTATION ORDER

Follow this order:

### Step 1

Inspect repository.

### Step 2

Locate existing regex classifier and understand its call graph.

### Step 3

Locate routing boundaries and existing `RequestType`.

### Step 4

Inspect `classifier_eval.rs`.

### Step 5

Design the classifier trait and requirement profile around existing code.

### Step 6

Implement regex adapter preserving existing behavior.

### Step 7

Implement ML classifier backend.

### Step 8

Implement structured requirement profile.

### Step 9

Integrate classifier into router using `Arc<dyn RequestClassifier>` or the repository-equivalent abstraction.

### Step 10

Implement configuration.

### Step 11

Implement regex fallback.

### Step 12

Integrate with `cold_start` and `switch`.

### Step 13

Preserve sticky routing during `continue`.

### Step 14

Update evaluator.

### Step 15

Add tests.

### Step 16

Add human demo.

### Step 17

Run:

```bash
cargo fmt --all
cargo check --workspace
cargo test --workspace
```

Then run the P2 evaluator.

Fix all compilation/test failures before considering the task complete.

---

# 40. DO NOT STOP AT "IT COMPILES"

Before finishing, verify all of the following:

* [ ] Existing regex behavior still works.
* [ ] ML classifier can be enabled.
* [ ] Regex remains default.
* [ ] Regex fallback works.
* [ ] Query is passed to classifier.
* [ ] Context is passed to classifier.
* [ ] Request type is returned.
* [ ] Complexity is returned in 1–5.
* [ ] Confidence is returned in 0–1.
* [ ] Requirement profile is generated.
* [ ] Conversation dependency is represented.
* [ ] Change impact is represented.
* [ ] Codebase/context size is represented where available.
* [ ] Cache affinity is represented where actual cache state is available.
* [ ] Model selection is NOT implemented inside classifier.
* [ ] No model capability benchmark matrix is hard-coded.
* [ ] No agent-selection redesign was introduced.
* [ ] Classification occurs at correct routing boundaries.
* [ ] `continue` remains sticky.
* [ ] Output is deterministic.
* [ ] Model loads only once.
* [ ] Latency is measured.
* [ ] Fallback is tested.
* [ ] Official evaluator still runs.
* [ ] Human demo works.
* [ ] No API keys are committed.
* [ ] No private data is committed.
* [ ] Existing default behavior is preserved.
* [ ] Tests pass.

---

# 41. FINAL OUTPUT FROM YOU

After implementation, provide me with:

## A. Files changed

List every modified/created file and explain why.

## B. Architecture

Show the final actual architecture based on the repository, not a theoretical architecture.

Use a diagram such as:

```text
User Request
     |
     v
Router
     |
     +---- Existing Regex Classifier
     |
     +---- ML Request Classifier
                |
                v
       Requirement Profile
                |
                v
          JSON Classification
                |
                v
       Downstream Routing
```

Adapt this to the actual implementation.

## C. JSON demo

Show at least 3 real examples using the implementation:

1. simple request
2. context-heavy request
3. high-impact but small code change

Show the actual JSON output produced.

## D. Commands

Give me the exact commands to:

1. build
2. test
3. run the official evaluator
4. run the human demo

## E. Configuration

Show the exact environment variables/configuration needed.

## F. Known limitations

Be honest.

Especially state:

* what the classifier understands,
* what it cannot currently infer,
* whether cache state is currently available from the router,
* whether codebase metadata is available,
* what remains for downstream model selection.

---

# 42. CRITICAL DESIGN PRINCIPLE

The final implementation should embody this separation:

```text
                    CLASSIFIER
                         |
                         v
             "What does this request
                    require?"
                         |
                         v
              Requirement Profile
                         |
                         v
                       JSON
                         |
                         v
                ROUTING LAYER
                         |
                         v
              "Which model/tier?"
```

The classifier does NOT answer the second question.

It only provides the information required to answer it.

---

# 43. FINAL PRIORITY ORDER

If there is a conflict between features, prioritize exactly in this order:

1. Official P2 interface compatibility
2. Existing Nasiko architecture
3. Correct request-type classification
4. Complexity classification
5. Confidence
6. Multidimensional requirement profile
7. Context/conversation awareness
8. Change-impact awareness
9. Regex fallback
10. Determinism
11. Correct routing-boundary integration
12. Tests
13. Official evaluator compatibility
14. Human demo
15. Additional enhancements

Do not sacrifice the required P2 behavior to implement optional sophistication.

The result should feel like a **real Nasiko OSS contribution**, not a standalone hackathon prototype bolted onto the repository.
