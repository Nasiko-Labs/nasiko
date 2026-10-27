# Ask WARROOM Specification

> **Phase 6: Contextual Question Answering Engine for Competitive Intelligence**

---

## 1. Purpose & Goals

**Ask WARROOM** provides interactive, natural-language question answering over currently loaded competitive intelligence signals. It acts as an analytical copilot for product managers, sales reps, marketers, and executive strategists seeking specific answers about competitor moves, strategic implications, or overall market dynamics.

### Key Capabilities:
- **Specific Inquiries**: Deep dives into a specific competitor move (e.g. *"Why does the Cashfree RiskShield launch matter to PayFlow?"*).
- **Landscape Synthesis**: Broad queries across all active signals (e.g. *"Summarize the current competitive landscape across all competitors."*).
- **Strategic Grounding**: Evaluates competitive moves in relation to PayFlow's capabilities (Payment Gateway, Smart Routing, FraudShield, Global Payouts) and customer segments.
- **Strict Evidence Grounding**: The system permits **zero hallucinated URLs**. All citations in the evidence section are verified against the input signals' `source_urls`.
- **Stateless & Deterministic**: Pure request-response semantics. No vector databases, embeddings, or persistent session storage are used; the context boundary is explicitly supplied in each request.

---

## 2. Architecture & Data Flow

```
User Question + Current Competitive Signals (from Scan)
                       ↓
         POST /api/ask (AskWarroomRequest)
                       ↓
   [Input Validation: question length, non-empty, signals]
                       ↓
   [Empty signals check] ──(if 0 signals)──> Immediate Insufficient Context Response
                       ↓
     FastAPI Backend (server/main.py)
                       ↓
      ask_warroom() (server/dronahq.py)
                       ↓
     DronaHQ Webhook (POST /v1/agent/run)
                       ↓
   Response Parsing & Normalization (_normalize_ask_response)
                       ↓
   [Evidence URL Filtering: valid_urls = dronahq_urls ∩ supplied_urls]
                       ↓
   [Fallback Engine: triggers on HTTP error, timeout, or malformed JSON]
                       ↓
         AskWarroomResponse (JSON)
```

---

## 3. API Contract (`POST /api/ask`)

### 3.1 Request Schema (`AskWarroomRequest`)

| Field | Type | Required | Validation / Description |
| :--- | :--- | :--- | :--- |
| `question` | `string` | Yes | 1 to 1000 characters. Cannot be empty or whitespace-only. |
| `signals` | `list[CompetitiveSignal]` | Yes | Competitive signals defining the context boundary. |
| `company` | `CompanyContext` | No | Target company profile. Defaults to PayFlow context if omitted. |

#### Example Request
```json
{
  "question": "Why does the Cashfree signal matter to PayFlow?",
  "signals": [
    {
      "competitor": "Cashfree",
      "category": "product",
      "headline": "Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds.",
      "summary": "Cashfree launched RiskShield to empower ecommerce merchants to curb cyber payment frauds in real-time.",
      "significance": "SIGNIFICANT",
      "overlap": {
        "products": ["FraudShield", "Payment Gateway"],
        "customer_segments": ["SMB", "Mid-Market"]
      },
      "impact": {
        "product": "HIGH",
        "sales": "MEDIUM",
        "marketing": "HIGH",
        "strategy": "LOW"
      },
      "recommended_actions": [
        "Audit FraudShield's rule engine against RiskShield's real-time blocking claims."
      ],
      "confidence": "high",
      "source_urls": [
        "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time",
        "https://www.cashfree.com/risk-shield-payment-gateway"
      ]
    }
  ]
}
```

### 3.2 Response Schema (`AskWarroomResponse`)

| Field | Type | Description |
| :--- | :--- | :--- |
| `answer` | `string` | Direct, synthesized answer grounded in the signal context. |
| `key_points` | `list[string]` | Structured takeaways, impact points, or recommended tracks. |
| `relevant_signals` | `list[RelevantSignalRef]` | References to signals that informed the answer (`competitor`, `headline`). |
| `evidence` | `AskWarroomEvidence` | Verified `source_urls` cited from the underlying research. |
| `confidence` | `string` | `"high"`, `"medium"`, or `"low"` based on evidence coverage. |
| `limitations` | `list[string]` | Caveats, missing data indicators, or reasoning limits. |

#### Example Response
```json
{
  "answer": "Cashfree's launch of 'RiskShield' directly impacts PayFlow because it competes head-to-head with PayFlow's FraudShield in real-time fraud mitigation for SMB and Mid-Market merchants.",
  "key_points": [
    "RiskShield introduces AI-powered fraud reduction claiming up to 40% fraud loss reduction.",
    "PayFlow's FraudShield faces direct product overlap in the SMB and Mid-Market ecommerce gateway space.",
    "Sales battlecards must emphasize PayFlow's zero-chargeback guarantee and native gateway integration."
  ],
  "relevant_signals": [
    {
      "competitor": "Cashfree",
      "headline": "Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds."
    }
  ],
  "evidence": {
    "source_urls": [
      "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time",
      "https://www.cashfree.com/risk-shield-payment-gateway"
    ]
  },
  "confidence": "high",
  "limitations": []
}
```

---

## 4. Strict Evidence Grounding & URL Filtering

To eliminate hallucinated links, `ask_warroom` enforces cryptographic citation integrity:
1. All `source_urls` present across the input `signals` list are collected into an authorized set `supplied_urls_set`.
2. Any URLs returned by the DronaHQ reasoning agent are cross-checked:
   ```python
   valid_evidence_urls = [u for u in candidate_urls if u in supplied_urls_set]
   ```
3. If DronaHQ cites an external link not present in the input signals, it is automatically discarded.
4. If no valid URLs are extracted from reasoning but relevant signals exist, the fallback populates evidence URLs directly from those relevant signals.

---

## 5. Resilience & Fallback Behavior

Ask WARROOM guarantees high availability under all network and execution conditions:

### 5.1 Empty Signals (`signals = []`)
When the user asks a question before running a scan or with zero signals loaded:
- The endpoint does not invoke external APIs.
- Immediately returns HTTP 200 with an honest advisory:
  ```json
  {
    "answer": "No competitive signals are currently loaded. Run a scan first or provide signal context to answer this question.",
    "key_points": ["Context is required to synthesize competitive answers."],
    "relevant_signals": [],
    "evidence": {"source_urls": []},
    "confidence": "low",
    "limitations": ["Insufficient context: no competitive signals provided."]
  }
  ```

### 5.2 Upstream Reasoning Failure (Timeout, HTTP 5xx, or Malformed JSON)
If DronaHQ times out or returns unparseable text:
- The backend catches the exception and constructs a deterministic fallback response.
- Scans `signals` for keyword matches against the question.
- Extracts headlines, summaries, and recommended actions from matching signals.
- Preserves genuine `source_urls` from matching signals.
- Marks `confidence: "low"` and documents the fallback in `limitations`.

---

## 6. Testing & Verification

Automated coverage in [`tests/test_ask.py`](file:///home/Krishna-Singh/WarRoom/tests/test_ask.py):
1. `test_ask_endpoint_valid_structured_json`: Valid signal and question return 200 with complete schema.
2. `test_ask_dronahq_json_string_response`: Handles DronaHQ stringified JSON response.
3. `test_ask_dronahq_markdown_fenced_response`: Handles markdown code-fenced responses.
4. `test_ask_empty_and_whitespace_question_validation`: Rejects empty/blank questions with HTTP 422.
5. `test_ask_oversized_question_validation`: Rejects questions >1000 chars with HTTP 422.
6. `test_ask_dronahq_http_error_fallback`: Graceful fallback on DronaHQ HTTP failure.
7. `test_ask_dronahq_timeout_fallback`: Graceful fallback on DronaHQ request timeout.
8. `test_ask_malformed_response_fallback`: Graceful fallback on unparseable responses.
9. `test_ask_empty_signals_insufficient_context`: Instant advisory on empty signals without calling DronaHQ.
10. `test_ask_evidence_url_filtering`: Rejects unverified URLs; keeps only supplied URLs.
11. `test_ask_relevant_signal_mapping_and_confidence`: Correctly maps relevant signals and confidence.
