# Business Logic & Reasoning Principles

> **The intelligence engine of WARROOM: How raw web results are transformed into actionable competitive signals without hallucination.**

---

## 1. Research Collection Strategy

Live research is the foundation of the WARROOM intelligence pipeline. Rather than issuing a generic query like `"Cashfree news"`, WARROOM dynamically constructs a context-rich search prompt tailored to the subject company:

```python
search_prompt = " ".join([competitor.name] + company.products + company.target_customers)
```

For the PayFlow benchmark:
- **Cashfree Query**: `Cashfree Payment Gateway Reconciliation FraudShield SMB Mid-Market`
- **Razorpay Query**: `Razorpay Payment Gateway Reconciliation FraudShield SMB Mid-Market`
- **PayU Query**: `PayU Payment Gateway Reconciliation FraudShield SMB Mid-Market`

### Why This Matters:
1. **High Signal-to-Noise Ratio**: Filtering out irrelevant corporate PR (e.g., office relocations, CSR initiatives) to surface product features, security tooling, fee structures, and merchant portal updates.
2. **Synchronous Speed**: Querying Anakin's synchronous `/v1/search` endpoint (default limit = 3) finishes within ~1.2 seconds per competitor, keeping total research latency under ~4 seconds.

---

## 2. The Evidence-Grounding Principle

WARROOM strictly adheres to the **Anti-Hallucination & Evidence-Grounding Principle**:

> **Rule 1: If a competitive event cannot be verified from the collected research snippets, it does not exist.**

- WARROOM never invents competitor product releases, pricing changes, executive departures, or partnership announcements.
- It never fabricates percentages (e.g., *"Cashfree captured 14% more market share"*) or dates unless explicitly stated in the source text.
- Every signal surfaced to the user must be backed by one or more verifiable public source URLs gathered in the research phase.

---

## 3. Signal Extraction & DronaHQ Reasoning

The collected research (9 source snippets, titles, URLs) is injected into the DronaHQ agent prompt. The reasoning agent is instructed to:
1. **Detect Changes**: Identify verified actions taken by the competitor (e.g., *"Launch of RiskShield"*).
2. **Classify Significance**: Determine whether the change is `SIGNIFICANT`, `INFORMATIONAL`, or `LOW_IMPACT`.
3. **Assess Product/Customer Overlap**: Compare against the user company's product lines (`Payment Gateway`, `Reconciliation`, `FraudShield`) and customer segments (`SMB`, `Mid-Market`).
4. **Identify Affected Internal Products**: Directly map the competitor's feature to the matching internal product (e.g., Cashfree *RiskShield* directly impacts PayFlow's *FraudShield*).
5. **Formulate Strategic Recommendations**: Draft talking tracks for sales battlecards, suggested questions for customer discovery, and roadmap priorities for product teams.

---

## 4. Signal Normalization Logic

Because LLM agent outputs can vary across schema versions, formatting styles, and serialization choices, the normalization layer in [`server/dronahq.py`](file:///home/Krishna-Singh/WarRoom/server/dronahq.py) provides multi-tier parsing:

### 4.1 De-Fencing and JSON Deserialization
Agent outputs wrapped in markdown backticks are cleaned before JSON parsing:
```python
def _clean_json_string(s: str) -> str:
    cleaned = s.strip()
    if cleaned.startswith("```"):
        lines = cleaned.splitlines()
        first_line = lines[0].strip()
        if first_line.startswith("```"):
            lines = lines[1:]
        if lines and lines[-1].strip() == "```":
            lines = lines[:-1]
        cleaned = "\n".join(lines).strip()
    return cleaned
```

### 4.2 Key Variant Resolution
The parser checks multiple possible container keys in order of precedence:
1. `parsed_output.get("competitive_signals")` (Agent's primary rich structured output)
2. `parsed_output.get("competitor_signals")` (Snake-case variant)
3. `parsed_output.get("competitorSignals")` (Camel-case variant)
4. `parsed_output.get("signals")` (Webhook standard schema array)

### 4.3 Field Mapping & Fact vs. Interpretation Distinction

WARROOM enforces a strict conceptual distinction between **grounded facts** and **strategic interpretations**:

| Field | Source Type | Description |
| :--- | :--- | :--- |
| **`competitor`** | Fact | Verified rival entity name (e.g. Cashfree, Razorpay, PayU). |
| **`headline`** | Fact | Summarizes the objective event from `change`, `changed`, or `title`. |
| **`summary`** | Mixed | Narrative explanation from `significance_explanation` or summary text. |
| **`source_urls`** | Fact | Direct public URLs collected by Anakin where the evidence was found. |
| **`significance`** | **Interpretation** | Strategic threat rating (`CRITICAL`, `SIGNIFICANT`, `INFORMATIONAL`, `NOISE`). |
| **`overlap`** | **Context Mapping** | Matches rival features against PayFlow’s `products` (e.g. FraudShield) and `customer_segments` (e.g. SMB, Mid-Market). |
| **`impact`** | **Interpretation** | Evaluates possible department disruption across Product, Sales, Marketing, and Strategy. |
| **`recommended_actions`** | **Action Guidance** | Pragmatic next steps framed as investigations or sales talk tracks (e.g. *"Investigate enhancements in..."*). |
| **`confidence`** | **Evidence Rating** | Categorical indicator (`"high"`, `"medium"`, `"low"`) reflecting evidence depth. |

If optional rich fields are not provided by DronaHQ, the normalization layer supplies safe defaults (`[]`, `None`, or empty models) without fabricating synthetic intelligence.

---

## 5. Competitor & Source Evidence Mapping

A signal is never an isolated text snippet. During normalization, WARROOM maintains a lookup dictionary:
```python
research_by_competitor = {
    "Cashfree": ["https://www.cashfree.com/news-room/...", "https://www.cashfree.com/risk-shield...", ...],
    "Razorpay": ["https://razorpay.com/blog/...", "https://razorpay.com/chargeback-shield...", ...],
    "PayU": ["https://corporate.payu.com/...", ...],
}
```

When a signal for `Cashfree` is normalized, the server attaches Cashfree’s verified source URLs to `signal.source_urls`. This ensures the user or frontend can inspect the raw source articles behind the AI’s synthesis with a single click.

---

## 6. Insufficient Evidence Protocol

If the collected web search results do not contain actionable competitive events, or if the reasoning agent determines that the evidence is inconclusive, DronaHQ returns:
```json
{
  "status": "insufficient_evidence",
  "signals": []
}
```

### Server Behavior:
1. WARROOM sets `reasoning.status = "insufficient_evidence"`.
2. WARROOM returns an empty list `signals = []`.
3. **Crucial Rule**: The system **NEVER** falls back to generic generated advice or synthetic signals to "fill the screen". An empty signal list with verified research is infinitely preferable to fabricated intelligence.

---

## 7. Failure Modes & Error Resilience

| Failure Scenario | Where Detected | System Behavior | Data Preservation |
| :--- | :--- | :--- | :--- |
| **Anakin API Timeout / Network Error** | `server/anakin.py` | Raises `AnakinTimeoutError` / `AnakinAPIError`; returns HTTP 504 or 502 with diagnostic detail | Scan aborts before reasoning step |
| **Missing API Key** | `server/config.py` | Raises `ConfigurationError`; returns HTTP 500 with descriptive error message | Prevents unauthenticated outgoing calls |
| **DronaHQ Webhook Timeout (>45s)** | `server/dronahq.py` | Catches `TimeoutException`; sets `reasoning.status = "error"`; `signals = []` | **All 9 Anakin research sources are preserved and returned to the client** |
| **DronaHQ HTTP 500 / 502** | `server/dronahq.py` | Catches `DronaHQAPIError`; sets `reasoning.status = "error"` | **All Anakin research sources preserved** |
| **Malformed Plain Text Response** | `server/dronahq.py` | Gracefully extracts the plain text into a signal headline rather than throwing an unhandled exception | Research preserved, partial signal extracted |
| **Empty Search Results** | `server/anakin.py` | Returns empty `results = []`; DronaHQ evaluates empty context and flags `insufficient_evidence` | Transparently reported to client |

---

## 8. Response Simulator Reasoning Principles (Phase 5)

The Response Simulator (`POST /api/simulate`) builds on the rich `CompetitiveSignal` model to generate structured decision-support options across Product, Sales, Marketing, and Strategy.

### 8.1 Exploratory Decision Support vs Autonomous Decisions
- **Decision Support**: The simulator formulates hypotheses, discovery questions, messaging angles, and areas for engineering investigation. It explicitly avoids making executive or roadmap commitments.
- **Evidence Boundary**: Only verified information in the supplied signal and source URLs is treated as ground truth.
- **Strict Anti-Fabrication Rules**:
  - The simulator never claims PayFlow possesses a capability unless present in the company context (`Payment Gateway`, `Reconciliation`, `FraudShield`).
  - The simulator never claims a competitor has a capability unless supported by verified source URLs.

### 8.2 Failure Resilience & Heuristic Fallback
When DronaHQ is unreachable or returns malformed text:
1. `simulate_response` does not raise an unhandled exception or crash the API.
2. An evidence-grounded fallback `ResponseSimulation` is generated using signal heuristics.
3. Original context (`competitor`, `headline`, `significance`) and all `source_urls` are strictly preserved.
4. Existing `recommended_actions` are partitioned into product investigation and sales talk tracks.
5. If `source_urls` is empty, `confidence` is forced to `"low"` and insufficient-evidence caveats are injected across all sections.

---

## 9. Ask WARROOM Reasoning Principles (Phase 6)

The Ask WARROOM engine (`POST /api/ask`) provides natural-language question answering bounded strictly by active `CompetitiveSignal` context.

### 9.1 Bounded Context & Zero-Shot Synthesis
- **No External Assumptions**: The agent is explicitly constrained to reason only over the provided `signals` and `company` profile.
- **Strict Evidence Grounding**: Hallucinated links are strictly forbidden. All returned evidence URLs are validated against `source_urls` in the input signals (`valid_urls = returned_urls ∩ input_urls`).
- **Target Company Context**: Questions are answered through the lens of PayFlow's capabilities (Payment Gateway, Smart Routing, FraudShield, Global Payouts) and customer segments (SMB, Mid-Market).

### 9.2 Immediate Insufficient Context Handling
- When `signals` is empty (`len(signals) == 0`), no external LLM call is made.
- An immediate, structured response is returned indicating that active competitive signals are required to formulate an answer.

### 9.3 Deterministic Fallback Engine
- If upstream reasoning fails (HTTP error, timeout, or malformed JSON), a deterministic fallback synthesizes keyword-matched signals.
- Preserves genuine headlines, summaries, recommended actions, and verified `source_urls`.
- Transparently documents the fallback event in `limitations`.

