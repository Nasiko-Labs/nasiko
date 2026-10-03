# Nasiko LLM Router — Request Classifier Datasets

This directory contains curated reference datasets for developing, validating, and testing request classifiers under Track P2 (Request Classification for Cost-Aware Routing).

## Dataset Files

- `classifier_train.jsonl`: 70 labelled training examples covering all 7 request types.
- `classifier_validation.jsonl`: 21 labelled validation examples covering all 7 request types.

Each line is a self-contained JSON object with the following schema:

```json
{
  "id": "tr-cg-001",
  "query": "write a python function to parse a csv file and return records as dicts",
  "context": "data processing pipeline",
  "request_type": "code_generation",
  "complexity": 4,
  "split_group": "csv_parser"
}
```

---

## 1. Request Types & Labelling Criteria

| Request Type | Description | Key Indicators / Patterns |
| :--- | :--- | :--- |
| `code_generation` | Authoring, fixing, refactoring, or generating executable code, scripts, migrations, or tests. | "write function", "implement algorithm", "fix bug", "add error handling", "create migration" |
| `code_understanding` | Explaining existing code, diagnosing error messages, walking through logic, or analyzing complexity. | "explain function", "walk me through code", "what does this error mean", "why is this failing" |
| `technical_design` | Architectural trade-offs, system component design, database schemas, protocol design, edge caching. | "how should I design", "architecture trade-offs", "multi-tenant schema", "system architecture" |
| `analytical_reasoning` | Quantitative calculations, probability, mathematical proofs, logic puzzles, statistics, equations. | "calculate probability", "solve equation", "prove by induction", "expected value", "Bayes theorem" |
| `writing` | Text drafting, emails, announcements, documentation, release notes, tone revisions, proposals. | "draft email", "write blog post", "rewrite paragraph", "compose response", "release notes" |
| `factual_lookup` | Direct factual questions, specifications, definitions, constants, trivia, reference inquiries. | "what is the capital of", "define HTTP status code", "when was X released", "atomic number" |
| `general` | Greetings, acknowledgements, small talk, meta-prompts, casual conversational banter. | "hello", "good morning", "thanks", "ok sounds good", "bye" |

---

## 2. Complexity Rubric (1–5 Scale)

The dataset adheres strictly to the project complexity rubric:

1. **Complexity 1 (Factual / Trivial)**: Direct factual queries, constants, definitions, and conversational greetings (`factual_lookup`, `general`).
2. **Complexity 2 (Basic Writing)**: Standard text composition, email drafting, status updates, release notes (`writing`).
3. **Complexity 3 (Code Understanding)**: Explaining functions, interpreting stack traces, reviewing single-file snippets (`code_understanding`).
4. **Complexity 4 (Complex Code & Analysis)**: Multi-step code generation, algorithm implementation, concurrency control, mathematical reasoning (`code_generation`, `analytical_reasoning`).
5. **Complexity 5 (System Architecture & Design)**: Distributed system architecture, multi-tenant database partitioning, cross-region trade-off analysis (`technical_design`).

---

## 3. Context Awareness & Ambiguity Resolution

Queries often contain ambiguous text (e.g., *"Fix this"*, *"What does this mean?"*, *"Please polish this"*). The classifier resolves intent by inspecting `context`:

- **Query**: `"Fix this"` | **Context**: `None` $\rightarrow$ `general` (Complexity 1)
- **Query**: `"Fix this"` | **Context**: `"pytest failed: TypeError: unsupported operand in calculate_total()"` $\rightarrow$ `code_generation` (Complexity 4)
- **Query**: `"What does this mean?"` | **Context**: `"compiler error[E0382]: use of moved value: data"` $\rightarrow$ `code_understanding` (Complexity 3)
- **Query**: `"Please polish this"` | **Context**: `"We found an issue in the billing module and fixed it."` $\rightarrow$ `writing` (Complexity 2)

---

## 4. Near-Duplicate Leakage Prevention

To guarantee strict evaluation integrity:
- Every example is tagged with a semantic `split_group` identifier.
- **Rule**: Near-duplicate queries, paraphrased prompts, and related scenario variants share the same `split_group` and are allocated **strictly to either train OR validation, never both**.
- Validation datasets test generalization against held-out intent clusters rather than literal surface rewording of training examples.
- Train IDs (`tr-*`) and Validation IDs (`val-*`) are strictly disjoint.

---

## 5. Development & Evaluation Scope Notice

> **Notice**: The current experimental backends (`AcrcClassifier`, `HostedClassifier`, `LocalClassifier`) implement deterministic confidence-gated routing and heuristic feature cascading. This dataset serves as a benchmark and development validation reference; the runtime does not perform in-process gradient descent or weight fine-tuning on this dataset.
