# API Reference

> **Complete specification for all HTTP endpoints exposed by the WARROOM FastAPI backend.**

Base URL: `http://127.0.0.1:8000`

---

## 1. Health Check Endpoint

Check service operational status and verify server availability.

- **Path**: `/api/health`
- **Method**: `GET`
- **Authentication**: None
- **Rate Limit**: Uncapped

### Request
No request parameters or body.

### Response
- **Status**: `200 OK`
- **Content-Type**: `application/json`

```json
{
  "status": "ok",
  "service": "warroom-api"
}
```

### Curl Example
```bash
curl -s http://127.0.0.1:8000/api/health
```

---

## 2. Company Configuration Endpoint

Validate and echo a target company's competitive context without database persistence.

- **Path**: `/api/company`
- **Method**: `POST`
- **Authentication**: None
- **Content-Type**: `application/json`

### Request Body (`CompanyContext`)
| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `name` | string | Yes | Name of the monitored company (e.g., `"PayFlow"`) |
| `products` | array of strings | No | Active product offerings (default: `[]`) |
| `target_customers` | array of strings | No | Target market segments (default: `[]`) |

```json
{
  "name": "PayFlow",
  "products": ["Payment Gateway", "Reconciliation", "FraudShield"],
  "target_customers": ["SMB", "Mid-Market"]
}
```

### Response
- **Status**: `200 OK`
- **Content-Type**: `application/json`

Returns the validated `CompanyContext` object.

### Curl Example
```bash
curl -s -X POST http://127.0.0.1:8000/api/company \
  -H "Content-Type: application/json" \
  -d '{
    "name": "PayFlow",
    "products": ["Payment Gateway", "Reconciliation", "FraudShield"],
    "target_customers": ["SMB", "Mid-Market"]
  }'
```

---

## 3. Competitive Scan Endpoint

Executes the end-to-end competitive intelligence pipeline: gathers live web search results via Anakin across monitored rivals, constructs an evidence context, dispatches reasoning to DronaHQ, and returns normalized competitive signals linked to verified source URLs.

- **Path**: `/api/scan`
- **Method**: `POST`
- **Authentication**: None
- **Content-Type**: `application/json`

### Request Body (`ScanRequest`, Optional)
If an empty JSON body `{}` is provided, the endpoint automatically uses the default benchmark profile (PayFlow vs Cashfree, Razorpay, PayU) from `data/company_context.json`.

| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `company` | `CompanyContext` | No | Overrides the default company profile |
| `competitors` | array of `CompetitorConfig` | No | List of competitors to scan (e.g. `[{"name": "Stripe"}]`) |

```json
{
  "company": {
    "name": "PayFlow",
    "products": ["Payment Gateway", "Reconciliation", "FraudShield"],
    "target_customers": ["SMB", "Mid-Market"]
  },
  "competitors": [
    {"name": "Cashfree"},
    {"name": "Razorpay"},
    {"name": "PayU"}
  ]
}
```

### Response Body (`ScanResponse`)
- **Status**: `200 OK`
- **Content-Type**: `application/json`

| Field | Type | Description |
| :--- | :--- | :--- |
| `company` | `CompanyContext` | Evaluated company profile |
| `research` | array of `CompetitorResearch` | Verified search results per competitor with titles, URLs, and snippets |
| `reasoning` | `DronaHQRun` | Execution status (`completed`, `pending`, `insufficient_evidence`, `error`) and thread/run IDs |
| `signals` | array of `CompetitiveSignal` | Normalized competitive intelligence signals with significance, overlap, impact, actions, confidence, and sources |

#### `CompetitiveSignal` Schema Details
| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `competitor` | string | Yes | Name of the competitor (e.g. `"Cashfree"`) |
| `category` | string \| null | No | Signal type (`"product"`, `"pricing"`, etc.) |
| `headline` | string \| null | No | High-level summary of the competitive event |
| `summary` | string \| null | No | Detailed explanation of the change and context |
| `significance` | string \| null | No | Strategic classification (`"CRITICAL"`, `"SIGNIFICANT"`, `"INFORMATIONAL"`, `"NOISE"`) |
| `overlap` | `SignalOverlap` | Yes | Overlapping internal offerings: `products` and `customer_segments` |
| `impact` | `DepartmentImpact` | Yes | Departmental ratings: `product`, `sales`, `marketing`, `strategy` |
| `recommended_actions` | array of strings | Yes | Actionable investigation recommendations or sales talk tracks |
| `confidence` | string \| null | No | Evidence confidence rating (`"high"`, `"medium"`, `"low"`) |
| `source_urls` | array of strings | Yes | Verified public evidence URLs gathered by Anakin |

```json
{
  "company": {
    "name": "PayFlow",
    "products": [
      "Payment Gateway",
      "Reconciliation",
      "FraudShield"
    ],
    "target_customers": [
      "SMB",
      "Mid-Market"
    ]
  },
  "research": [
    {
      "competitor": "Cashfree",
      "results": [
        {
          "title": "Cashfree Payments Launches 'RiskShield' to Empower Merchants Curb Cyber Payment Frauds",
          "url": "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time",
          "snippet": "Cashfree Payments has launched RiskShield, a risk management solution aimed at curbing cyber payment frauds in real-time...",
          "date": null,
          "last_updated": null
        },
        {
          "title": "Riskshield: Powerful Fraud & Chargeback Protection",
          "url": "https://www.cashfree.com/risk-shield-payment-gateway",
          "snippet": "RiskShield Fraud-Free Payments, Guaranteed Peace of Mind. 80% Reduction in losses due to payment fraud...",
          "date": null,
          "last_updated": null
        },
        {
          "title": "RiskShield Overview",
          "url": "https://www.cashfree.com/docs/payments/risk-shield/overview",
          "snippet": "Use the Cashfree RiskShield Analytics Dashboard to evaluate your fraud profile...",
          "date": null,
          "last_updated": null
        }
      ]
    },
    {
      "competitor": "Razorpay",
      "results": [
        {
          "title": "How Can Payment Gateways Help You Reduce the Risk of Payment Fraud?",
          "url": "https://razorpay.com/blog/payment-gateways-reduce-fraud-risk",
          "snippet": "Payment fraud is a harsh reality in today's digital world...",
          "date": null,
          "last_updated": null
        },
        {
          "title": "Chargeback Shield - Zero Chargebacks, Guaranteed",
          "url": "https://razorpay.com/chargeback-shield",
          "snippet": "Introducing Chargeback Shield. Say goodbye to chargebacks forever...",
          "date": null,
          "last_updated": null
        },
        {
          "title": "How Razorpay's Payment Gateway Can Support Small Businesses",
          "url": "https://razorpay.com/blog/payment-gateway-support-for-small-businesses",
          "snippet": "Why Small Businesses Need a Reliable Payment Gateway...",
          "date": null,
          "last_updated": null
        }
      ]
    },
    {
      "competitor": "PayU",
      "results": [
        {
          "title": "Advanced Anti-Fraud Solutions for Online Payment Processing",
          "url": "https://corporate.payu.com/payment-security/anti-fraud-solutions-for-online-payment-processing",
          "snippet": "Combat fraud with PayU's multi-layered security and advanced anti-fraud tools...",
          "date": null,
          "last_updated": null
        },
        {
          "title": "The Most Common Forms of Payment Fraud and How to Avoid Them",
          "url": "https://corporate.payu.com/blog/the-most-common-forms-of-payment-fraud-and-how-to-avoid-them",
          "snippet": "Online payments continue to surge in popularity across the globe...",
          "date": null,
          "last_updated": null
        },
        {
          "title": "How PayU Prevents Online Payment Frauds",
          "url": "https://payu.in/blog/how-payu-prevents-online-payment-frauds",
          "snippet": "Learn about the security mechanisms PayU implements...",
          "date": null,
          "last_updated": null
        }
      ]
    }
  ],
  "reasoning": {
    "status": "completed",
    "thread_id": "ff29f551-ffca-42d2-9e64-4b3ffdf35dea",
    "run_id": "2b916f71-b240-42c1-9fbc-435921d56f0f",
    "message": "Agent run completed successfully. See 'response' for execution output."
  },
  "signals": [
    {
      "competitor": "Cashfree",
      "category": "product",
      "headline": "Launch of RiskShield, a real-time risk management solution for payment gateways.",
      "summary": "The introduction of RiskShield signifies Cashfree's commitment to enhancing security for merchants, potentially attracting businesses concerned with fraud...",
      "significance": "SIGNIFICANT",
      "overlap": {
        "products": ["FraudShield"],
        "customer_segments": ["SMB", "Mid-Market"]
      },
      "impact": {
        "product": "MEDIUM",
        "sales": "LOW",
        "marketing": "HIGH",
        "strategy": "LOW"
      },
      "recommended_actions": [
        "Investigate enhancements in fraud prevention and risk management features within the FraudShield product to closely match industry leaders’ offerings."
      ],
      "confidence": "high",
      "source_urls": [
        "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time",
        "https://www.cashfree.com/risk-shield-payment-gateway",
        "https://www.cashfree.com/docs/payments/risk-shield/overview"
      ]
    },
    {
      "competitor": "Razorpay",
      "category": "product",
      "headline": "Introduction of Chargeback Shield leveraging machine learning to analyze transaction patterns for fraud detection.",
      "summary": "Razorpay's Chargeback Shield enhances their fraud prevention offerings and improves the protection against chargebacks...",
      "significance": "INFORMATIONAL",
      "overlap": {
        "products": ["FraudShield"],
        "customer_segments": ["Mid-Market"]
      },
      "impact": {
        "product": "MEDIUM",
        "sales": "MEDIUM",
        "marketing": "MEDIUM",
        "strategy": "LOW"
      },
      "recommended_actions": [
        "Investigate enhancements in fraud prevention and risk management features within the FraudShield product to closely match industry leaders’ offerings."
      ],
      "confidence": "high",
      "source_urls": [
        "https://razorpay.com/blog/payment-gateways-reduce-fraud-risk",
        "https://razorpay.com/chargeback-shield",
        "https://razorpay.com/blog/payment-gateway-support-for-small-businesses"
      ]
    },
    {
      "competitor": "PayU",
      "category": "product",
      "headline": "Enhancements to anti-fraud solutions, integrating AI-powered monitoring and detection capabilities.",
      "summary": "PayU's commitment to robust anti-fraud mechanisms positions it as a strong competitor for businesses needing reliable fraud protection...",
      "significance": "NOISE",
      "overlap": {
        "products": ["FraudShield"],
        "customer_segments": ["SMB"]
      },
      "impact": {
        "product": "LOW",
        "sales": "LOW",
        "marketing": "MEDIUM",
        "strategy": "LOW"
      },
      "recommended_actions": [
        "Investigate enhancements in fraud prevention and risk management features within the FraudShield product to closely match industry leaders’ offerings."
      ],
      "confidence": "high",
      "source_urls": [
        "https://corporate.payu.com/payment-security/anti-fraud-solutions-for-online-payment-processing",
        "https://corporate.payu.com/blog/the-most-common-forms-of-payment-fraud-and-how-to-avoid-them",
        "https://payu.in/blog/how-payu-prevents-online-payment-frauds"
      ]
    }
  ]
}
```

### Error Responses
- **`500 Internal Server Error`**: Missing required environment variable (`ANAKIN_API_KEY`, `DRONAHQ_WEBHOOK_URL`, or `DRONAHQ_API_KEY`).
- **`502 Bad Gateway`**: Anakin Search API returned an HTTP error status code or sent an invalid response body.
- **`504 Gateway Timeout`**: Anakin Search API query timed out after 20 seconds.

### Curl Example
```bash
curl -s -X POST http://127.0.0.1:8000/api/scan \
  -H "Content-Type: application/json" \
  -d '{}'
```

---

## 4. Response Simulator Endpoint

Generates structured, evidence-grounded response options across Product, Sales, Marketing, and Strategy for a single `CompetitiveSignal`.

- **Path**: `/api/simulate`
- **Method**: `POST`
- **Authentication**: None
- **Content-Type**: `application/json`

### Request Body (`SimulateRequest`)
| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `signal` | `CompetitiveSignal` | Yes | The competitive signal to simulate against |
| `company` | `CompanyContext` | No | Monitored company profile (defaults to PayFlow) |

```json
{
  "signal": {
    "competitor": "Cashfree",
    "headline": "Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds.",
    "category": "product",
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
      "Investigate whether FraudShield needs real-time velocity checking."
    ],
    "confidence": "high",
    "source_urls": [
      "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time"
    ]
  }
}
```

### Response (`ResponseSimulation`)
- **Status**: `200 OK`
- **Content-Type**: `application/json`

Returns structured response options across `product`, `sales`, `marketing`, and `strategy`, along with preserved `evidence.source_urls` and `confidence`.

### Curl Example
```bash
curl -s -X POST http://127.0.0.1:8000/api/simulate \
  -H "Content-Type: application/json" \
  -d '{
    "signal": {
      "competitor": "Cashfree",
      "headline": "RiskShield Launched",
      "source_urls": ["https://cashfree.com/riskshield"]
    }
  }'
```

---

## 5. Ask WARROOM Endpoint

Interactive natural-language question answering grounded in active competitive signals.

- **Path**: `/api/ask`
- **Method**: `POST`
- **Authentication**: None
- **Content-Type**: `application/json`

### Request Body (`AskWarroomRequest`)
| Field | Type | Required | Description |
| :--- | :--- | :--- | :--- |
| `question` | string | Yes | Question to ask (1 to 1000 characters, non-empty, non-whitespace) |
| `signals` | array of `CompetitiveSignal` | Yes | Active competitive signals defining the context boundary |
| `company` | `CompanyContext` | No | Target company profile (defaults to PayFlow) |

```json
{
  "question": "Why does the Cashfree signal matter to PayFlow?",
  "signals": [
    {
      "competitor": "Cashfree",
      "headline": "Launch of 'RiskShield', a real-time risk management solution for cyber payment frauds.",
      "category": "product",
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
        "Audit FraudShield rule engine against RiskShield claims."
      ],
      "confidence": "high",
      "source_urls": [
        "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time"
      ]
    }
  ]
}
```

### Response (`AskWarroomResponse`)
- **Status**: `200 OK`
- **Content-Type**: `application/json`

| Field | Type | Description |
| :--- | :--- | :--- |
| `answer` | string | Direct contextual answer synthesized from the signals |
| `key_points` | array of strings | Bullet points summarizing strategic or tactical takeaways |
| `relevant_signals` | array of objects | Referenced signals (`competitor`, `headline`) |
| `evidence` | object (`AskWarroomEvidence`) | `source_urls` strictly filtered against supplied signal URLs |
| `confidence` | string | `"high"`, `"medium"`, or `"low"` |
| `limitations` | array of strings | Documented boundaries, caveats, or fallback notes |

### Curl Example
```bash
curl -s -X POST http://127.0.0.1:8000/api/ask \
  -H "Content-Type: application/json" \
  -d '{
    "question": "Why does Cashfree matter?",
    "signals": [
      {
        "competitor": "Cashfree",
        "headline": "RiskShield Launched",
        "source_urls": ["https://cashfree.com/riskshield"]
      }
    ]
  }'
```

