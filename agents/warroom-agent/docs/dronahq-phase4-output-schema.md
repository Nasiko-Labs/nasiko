# DronaHQ Phase 4 Output Schema Specification

> **Proposed standard JSON Schema and agent prompt guidelines for the DronaHQ competitive reasoning agent.**

---

## 1. Overview & Purpose

This document specifies the target JSON Schema and prompt instructions for the DronaHQ reasoning agent to formally standardize the rich competitive intelligence output consumed by WARROOM Phase 4.

While the WARROOM backend normalization layer ([`server/dronahq.py`](file:///home/Krishna-Singh/WarRoom/server/dronahq.py)) already tolerates multiple field name variants (`competitive_signals`, `change`, `changed`, `affected_internal_products`, `affected_customer_segments`, etc.), configuring DronaHQ's webhook **Response Schema** with this exact draft will ensure consistent, schema-validated responses across all agent executions.

---

## 2. Proposed DronaHQ JSON Schema (Draft 2020-12)

Configure this schema in the DronaHQ Webhook Settings under **Response Type: Standard** → **JSON Schema**:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "WARROOM Rich Competitive Intelligence Response",
  "description": "Structured competitive intelligence signals with significance, overlap, impact, and action recommendations.",
  "type": "object",
  "properties": {
    "status": {
      "type": "string",
      "enum": ["success", "insufficient_evidence"]
    },
    "competitive_signals": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "competitor": {
            "type": "string",
            "description": "Name of the competitor (e.g. Cashfree, Razorpay, PayU)."
          },
          "change": {
            "type": "string",
            "description": "Factual description of what the competitor changed or launched."
          },
          "signal_classification": {
            "type": "object",
            "properties": {
              "type": {
                "type": "string",
                "enum": ["product", "pricing", "positioning", "partnership", "market_expansion"]
              },
              "significance": {
                "type": "string",
                "enum": ["CRITICAL", "SIGNIFICANT", "INFORMATIONAL", "NOISE"]
              }
            },
            "required": ["type", "significance"]
          },
          "significance_explanation": {
            "type": "string",
            "description": "Explanation of why this change matters to the industry and company."
          },
          "affected_products": {
            "type": "array",
            "items": { "type": "string" },
            "description": "PayFlow products with direct or indirect feature overlap (e.g. FraudShield, Payment Gateway, Reconciliation)."
          },
          "affected_segments": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Target customer segments overlapping (e.g. SMB, Mid-Market)."
          },
          "impact_assessment": {
            "type": "object",
            "properties": {
              "product": { "type": "string", "enum": ["HIGH", "MEDIUM", "LOW", "NONE"] },
              "sales": { "type": "string", "enum": ["HIGH", "MEDIUM", "LOW", "NONE"] },
              "marketing": { "type": "string", "enum": ["HIGH", "MEDIUM", "LOW", "NONE"] },
              "strategy": { "type": "string", "enum": ["HIGH", "MEDIUM", "LOW", "NONE"] }
            },
            "required": ["product", "sales", "marketing", "strategy"]
          },
          "recommended_actions": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Concrete investigation steps or sales talk tracks for internal teams."
          },
          "confidence": {
            "type": "string",
            "enum": ["high", "medium", "low"],
            "description": "Strength of evidence supporting this signal."
          }
        },
        "required": [
          "competitor",
          "change",
          "signal_classification",
          "significance_explanation",
          "affected_products",
          "affected_segments",
          "impact_assessment"
        ]
      }
    }
  },
  "required": ["status", "competitive_signals"],
  "additionalProperties": true
}
```

---

## 3. Example Valid DronaHQ Response

```json
{
  "status": "success",
  "competitive_signals": [
    {
      "competitor": "Cashfree",
      "change": "Launch of RiskShield, a real-time risk management solution for payment gateways.",
      "signal_classification": {
        "type": "product",
        "significance": "SIGNIFICANT"
      },
      "significance_explanation": "RiskShield aims to reduce fraudulent activities by up to 40% and integrates ML algorithms for real-time transaction monitoring.",
      "affected_products": ["FraudShield"],
      "affected_segments": ["SMB", "Mid-Market"],
      "impact_assessment": {
        "product": "MEDIUM",
        "sales": "LOW",
        "marketing": "HIGH",
        "strategy": "LOW"
      },
      "recommended_actions": [
        "Investigate enhancements in fraud prevention within FraudShield to match market capabilities.",
        "Sales talk track: Emphasize PayFlow dedicated onboarding and verified fraud resolution SLAs."
      ],
      "confidence": "high"
    }
  ]
}
```

---

## 4. Field Meanings & Definitions

| Field | Meaning | Interpretation vs Fact |
| :--- | :--- | :--- |
| `competitor` | Name of the rival company | **Fact** (from monitored list) |
| `change` | Objective statement of what was released or changed | **Fact** (must be grounded in research snippets) |
| `signal_classification.type` | Category of the development (`product`, `pricing`, etc.) | **Categorization** |
| `signal_classification.significance` | Threat level (`CRITICAL`, `SIGNIFICANT`, `INFORMATIONAL`, `NOISE`) | **Strategic Interpretation** |
| `significance_explanation` | Why the development matters to PayFlow | **Strategic Interpretation** |
| `affected_products` | Internal products directly competing with the rival feature | **Context Mapping** |
| `affected_segments` | Customer segments targeted by both PayFlow and the rival | **Context Mapping** |
| `impact_assessment` | Ratings across Product, Sales, Marketing, and Strategy | **Departmental Assessment** |
| `recommended_actions` | Pragmatic steps framed as investigations or battlecards | **Action Guidance** |
| `confidence` | Assessment of research grounding strength | **Evidence Confidence** |

---

## 5. Recommended DronaHQ Agent Instruction Updates

Add or update the following instructions in the DronaHQ Agent system prompt:

```text
You are the WARROOM Competitive Intelligence Reasoning Agent.
Analyze the provided competitor research and compare it against the company's product lines and target segments.

STRICT REASONING RULES:
1. Grounding: Rely strictly on the provided competitor research snippets and URLs. Do not fabricate events, percentages, or dates.
2. If the research contains no meaningful recent competitive changes, return:
   {"status": "insufficient_evidence", "competitive_signals": []}
3. For each genuine signal:
   - Identify the exact competitor name.
   - Summarize the factual change in the 'change' field.
   - Classify significance as CRITICAL, SIGNIFICANT, INFORMATIONAL, or NOISE.
   - Identify affected internal products from the company's profile (e.g. FraudShield, Payment Gateway, Reconciliation).
   - Identify affected customer segments (e.g. SMB, Mid-Market).
   - Assess impact ratings (HIGH, MEDIUM, LOW, NONE) for product, sales, marketing, and strategy.
   - Suggest 1-2 actionable investigation recommendations or sales objection talk tracks.
   - Assign evidence confidence as 'high' (well-documented in snippets), 'medium' (partially documented), or 'low'.
4. Output strictly valid JSON conforming to the output schema.
```

---

## 6. Backward Compatibility Notes

- The WARROOM backend parser in [`server/dronahq.py`](file:///home/Krishna-Singh/WarRoom/server/dronahq.py) supports:
  - Both top-level and signal-level `recommended_actions` and `sales_battlecard`.
  - Both `change` and `changed`.
  - Both `affected_products` and `affected_internal_products`.
  - Both `affected_segments` and `affected_customer_segments`.
  - Legacy basic signal arrays (`[{"headline": "...", "summary": "..."}]`).
- Updating DronaHQ's schema in the UI is safe and will not cause regressions in existing code.
