# Response Simulator Specification

> **Phase 5: Decision-Support Simulation Engine for Competitive Moves**

---

## 1. Purpose & Goals

The **Response Simulator** enables product, sales, marketing, and executive leadership teams to explore structured, evidence-grounded response options when a competitor makes a strategic or product move.

### What the Simulator Is:
- **A Decision-Support Engine**: Formulates exploratory response options, investigation questions, talking tracks, and positioning angles.
- **Strictly Evidence-Grounded**: Bound entirely by verified web research evidence and confirmed internal company capabilities.
- **Transparent**: Highlights caveats, required validations, and missing technical proof points.

### What the Simulator Is NOT:
- **Not an Autonomous Decision-Maker**: It does not decide company policy or commit roadmap resources.
- **Not a Hallucination Engine**: It does not invent competitor features, market share, revenue numbers, customer quotes, pricing, or unconfirmed internal capabilities.

---

## 2. Architecture & Pipeline Placement

```
Anakin.io (Live Web Research)
       ↓
Research Evidence (Source URLs & Snippets)
       ↓
DronaHQ Reasoning (Signal Extraction)
       ↓
CompetitiveSignal (Central Intelligence Object)
       ↓
Response Simulator (POST /api/simulate)
       ↓
Structured Response Options (Product, Sales, Marketing, Strategy)
       ↓
(Future: Dashboard UI / Ask WARROOM / Nasiko Actions)
```

The simulator consumes an existing `CompetitiveSignal` without creating a competing data abstraction.

---

## 3. Input Contract (`SimulateRequest`)

- **Path**: `/api/simulate`
- **Method**: `POST`
- **Headers**: `Content-Type: application/json`

### Schema

| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `signal` | `CompetitiveSignal` | Yes | The rich competitive signal to simulate against. |
| `company` | `CompanyContext` | No | Target company profile. If omitted, defaults to PayFlow. |

### Example Request

```json
{
  "signal": {
    "competitor": "Cashfree",
    "category": "product",
    "headline": "Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds.",
    "summary": "Cashfree launched RiskShield to empower ecommerce merchants to block fraudulent transactions in real-time.",
    "significance": "SIGNIFICANT",
    "overlap": {
      "products": ["FraudShield", "Payment Gateway"],
      "customer_segments": ["SMB", "Mid-Market"]
    },
    "impact": {
      "product": "HIGH",
      "sales": "MEDIUM",
      "marketing": "MEDIUM",
      "strategy": "LOW"
    },
    "recommended_actions": [
      "Investigate machine learning capabilities that could enhance FraudShield's real-time transaction monitoring.",
      "Sales talk track: Highlight PayFlow's native gateway integration and zero chargeback guarantee."
    ],
    "confidence": "high",
    "source_urls": [
      "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time",
      "https://www.cashfree.com/risk-shield-payment-gateway"
    ]
  }
}
```

---

## 4. Output Contract (`ResponseSimulation`)

### Suggested Schema Structure

```
ResponseSimulation
├── signal_context
│   ├── competitor
│   ├── headline
│   ├── category
│   └── significance
├── product
│   ├── actions
│   ├── investigation_questions
│   └── caveats
├── sales
│   ├── trigger
│   ├── talk_track
│   ├── questions
│   └── caveats
├── marketing
│   ├── actions
│   ├── messaging_angles
│   └── caveats
├── strategy
│   ├── questions
│   ├── actions
│   └── caveats
├── evidence
│   └── source_urls
└── confidence
```

### Sub-Model Definitions

1. **`signal_context` (`SignalContextSummary`)**:
   - `competitor`: Name of competitor.
   - `headline`: Summary title of the move.
   - `category`: Signal category (`product`, `pricing`, etc.).
   - `significance`: Assessed impact (`CRITICAL`, `SIGNIFICANT`, etc.).
2. **`product` (`ProductResponse`)**:
   - `actions`: Technical or product areas to investigate.
   - `investigation_questions`: Key questions to evaluate before development.
   - `caveats`: Assumptions that should not be made without validation.
3. **`sales` (`SalesResponse`)**:
   - `trigger`: Situations when a prospect raises this competitive move.
   - `talk_track`: Evidence-grounded talk track re-anchoring on verified strengths.
   - `questions`: Discovery questions for sales reps to ask prospects.
   - `caveats`: Disclaimers against unverified feature parity claims.
4. **`marketing` (`MarketingResponse`)**:
   - `actions`: Positioning and messaging audit actions.
   - `messaging_angles`: Themes and value propositions to validate.
   - `caveats`: Guidance against unsupported superiority claims.
5. **`strategy` (`StrategyResponse`)**:
   - `questions`: Core strategic questions for executive leadership.
   - `actions`: Recommended investigation and monitoring actions.
   - `caveats`: Reminders that output provides decision support, not automatic strategy.
6. **`evidence` (`SimulationEvidence`)**:
   - `source_urls`: Preserved list of verified public URLs supporting the signal.
7. **`confidence`**: Assessment rating (`high`, `medium`, `low`).

### Example Response

```json
{
  "signal_context": {
    "competitor": "Cashfree",
    "headline": "Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds.",
    "category": "product",
    "significance": "SIGNIFICANT"
  },
  "product": {
    "actions": [
      "Investigate potential fields of enhancement for PayFlow's FraudShield, particularly in machine learning and real-time capabilities."
    ],
    "investigation_questions": [
      "What specific machine learning enhancements can be integrated into FraudShield?",
      "What are the current limitations of FraudShield in real-time fraud detection?",
      "How does FraudShield's performance compare to 'RiskShield' in terms of effectiveness?"
    ],
    "caveats": [
      "Enhancements should be aligned with customer needs and competitive landscape."
    ]
  },
  "sales": {
    "trigger": "A prospect expresses concerns about payment fraud and mentions a competitor's new solution.",
    "talk_track": "PayFlow's FraudShield has been designed with proactive measures against payment fraud, and we continuously enhance our capabilities to provide top-level security for our clients.",
    "questions": [
      "Have you evaluated what specific fraud detection metrics are important for your business?",
      "What challenges have you faced with fraud prevention solutions in the past?",
      "How critical is real-time fraud detection to your operations?"
    ],
    "caveats": [
      "Be cautious about making direct comparisons without clear performance data."
    ]
  },
  "marketing": {
    "actions": [
      "Consider promoting advancements in FraudShield capabilities focused on AI and real-time detection features."
    ],
    "messaging_angles": [
      "Highlight PayFlow's commitment to leveraging technology for enhanced security.",
      "Emphasize customer success stories related to fraud prevention."
    ],
    "caveats": [
      "Ensure messaging is clear and evidence-backed to avoid over-promising capabilities."
    ]
  },
  "strategy": {
    "questions": [
      "How can we differentiate PayFlow's FraudShield from Cashfree's RiskShield?",
      "What alliances or partnerships can we explore to enhance FraudShield's capabilities?"
    ],
    "actions": [
      "Evaluate the competitive landscape to identify key differentiators."
    ],
    "caveats": [
      "Strategic decisions should be based on a thorough market analysis."
    ]
  },
  "evidence": {
    "source_urls": [
      "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time",
      "https://www.cashfree.com/risk-shield-payment-gateway",
      "https://www.cashfree.com/docs/payments/risk-shield/overview"
    ]
  },
  "confidence": "high"
}
```

---

## 5. DronaHQ Integration & Prompt Role

The simulator reuses the existing DronaHQ agent webhook (`DRONAHQ_WEBHOOK_URL`). It does **not** introduce a second LLM provider or multi-hop agent chains.

When `POST /api/simulate` is called, exactly **one** call is dispatched to DronaHQ with:
- PayFlow company context (products, target customers).
- The full `CompetitiveSignal` object (headline, summary, overlap, departmental impact, recommended actions, source URLs).
- Explicit system instructions:
  > *"You are generating response OPTIONS for investigation and decision support. Do not make the decision for the user. Do not invent facts. Use only information contained in the supplied signal and evidence. Clearly distinguish known evidence, reasonable response option, and question requiring validation. Do not claim that PayFlow has a capability unless it is present in the supplied company context. Do not claim the competitor has a capability unless supported by the supplied evidence."*

---

## 6. Evidence Grounding & Insufficient Evidence Handling

- **Source URL Preservation**: Every simulation preserves the signal's verified `source_urls` in `evidence.source_urls`.
- **No Fabricated Sources**: The backend never generates placeholder or dummy URLs.
- **Missing Source URLs**: If a signal is provided with `source_urls: []`:
  - `confidence` is forced to `"low"`.
  - Every departmental section includes an explicit caveat: *"Evidence is insufficient: no verified source URLs provided in competitive signal."*

---

## 7. Fallback & Resilience Behavior

If DronaHQ encounters a network error, timeout (45s), HTTP 5xx, or returns unparseable text:
- The endpoint does **not** crash.
- It returns an evidence-grounded fallback `ResponseSimulation`.
- It preserves the original `signal_context` and all `source_urls`.
- It reuses existing `recommended_actions` from the signal (e.g., existing product investigation steps and sales battlecard talk tracks).
- It injects explicit transparency notes into `caveats` (e.g., *"AI simulation fallback: Network error communicating with DronaHQ"*).

---

## 8. Limitations & Future Scope

1. **Synchronous Webhook Execution**: DronaHQ webhook operates in Standard response mode with a 45s HTTP timeout.
2. **Single-Signal Scope**: `/api/simulate` evaluates one signal at a time. Cross-signal portfolio analysis is reserved for future milestones.
3. **No UI Integration in Phase 5**: In accordance with the Phase 5 specification, frontend changes are frozen until subsequent phases.
