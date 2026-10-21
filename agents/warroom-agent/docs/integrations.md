# External Integrations

> **Specifications, authentication protocols, and interaction contracts for external AI and search providers.**

---

## 1. Anakin.io Search API

### 1.1 Purpose
Anakin.io provides synchronous web search and retrieval, acting as the live evidence collection engine for WARROOM. It allows the system to ground competitive signals in factual, current public web content rather than relying on an LLM's static training cutoff.

### 1.2 Endpoint & Protocol
- **Endpoint**: `https://api.anakin.io/v1/search`
- **HTTP Method**: `POST`
- **Timeout**: 20.0 seconds
- **Authentication**: Custom header `X-API-Key: <ANAKIN_API_KEY>`

### 1.3 Request Format
```http
POST /v1/search HTTP/1.1
Host: api.anakin.io
Content-Type: application/json
X-API-Key: [REDACTED_API_KEY]

{
  "prompt": "Cashfree Payment Gateway Reconciliation FraudShield SMB Mid-Market",
  "limit": 3
}
```

- `prompt` (*string*, required): Targeted keywords including competitor name, monitored products, and customer segments.
- `limit` (*integer*, optional, default: 3): Number of top search results to return. Configurable via `ANAKIN_LIMIT` in `.env`.

### 1.4 Response Structure
```json
{
  "results": [
    {
      "title": "Cashfree Payments Launches 'RiskShield' to Empower Merchants Curb Cyber Payment Frauds",
      "url": "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield-to-empower-merchants-curb-cyber-payment-frauds-in-real-time",
      "snippet": "Cashfree Payments has launched RiskShield, a risk management solution aimed at curbing cyber payment frauds in real-time. The solution helps merchants mitigate risk...",
      "date": null,
      "last_updated": null
    }
  ]
}
```

### 1.5 Usage in WARROOM
- The backend iterates through all configured competitors sequentially or concurrently.
- For 3 competitors (`Cashfree`, `Razorpay`, `PayU`), Anakin produces 9 total web sources.
- Extracted `title`, `url`, `snippet`, and optional dates are converted into `ResearchResult` models and passed to the reasoning layer.

---

## 2. DronaHQ Reasoning Agent

### 2.1 Purpose
DronaHQ provides the agentic workflow and LLM reasoning layer. It receives the collected web research alongside the user company's internal product context, evaluates competitive significance, detects feature overlap, and outputs structured strategic signals.

### 2.2 Webhook Architecture & Response Modes
- **Endpoint**: `POST <DRONAHQ_WEBHOOK_URL>` (Configured in `.env`)
- **HTTP Method**: `POST`
- **Timeout**: 45.0 seconds
- **Authentication**: Custom header `api-key: <DRONAHQ_API_KEY>`
- **Response Mode**: **Standard** (Synchronous execution)

> [!IMPORTANT]
> DronaHQ agents configured with **Standard** response mode execute the agent workflow synchronously during the webhook HTTP request and return the final execution output inside the `"response"` field. The earlier asynchronous mode (which returned `"status: pending"` with `thread_id` and `run_id`) has been resolved. WARROOM does NOT poll or invent artificial polling endpoints.

### 2.3 Webhook Request Payload
```json
{
  "message": "Analyze the following competitor research for PayFlow (Products: Payment Gateway, Reconciliation, FraudShield; Segments: SMB, Mid-Market). Extract meaningful competitive signals based only on the provided evidence.",
  "company": {
    "name": "PayFlow",
    "products": ["Payment Gateway", "Reconciliation", "FraudShield"],
    "target_customers": ["SMB", "Mid-Market"]
  },
  "competitors": [
    {
      "name": "Cashfree",
      "research": [
        {
          "title": "Cashfree Payments Launches 'RiskShield'...",
          "url": "https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield...",
          "snippet": "Cashfree Payments has launched RiskShield, a risk management solution..."
        }
      ]
    }
  ]
}
```

### 2.4 Verified Webhook Response Wrapper
```json
{
  "success": true,
  "thread_id": "a945643b-1d54-42d9-9fb5-c28729668125",
  "run_id": "0eced073-00b1-4c5b-baf7-817b029974eb",
  "message": "Agent run completed successfully. See 'response' for execution output.",
  "response": "```json\n{\n  \"competitive_signals\": [\n    {\n      \"competitor\": \"Cashfree\",\n      \"change\": \"Launch of RiskShield, a real-time risk management solution for payment gateways.\",\n      \"signal_classification\": {\n        \"type\": \"product\",\n        \"significance\": \"SIGNIFICANT\"\n      },\n      \"significance_explanation\": \"RiskShield aims to reduce fraudulent activities by up to 40%...\",\n      \"affected_products\": [\"FraudShield\"],\n      \"affected_segments\": [\"SMB\", \"Mid-Market\"]\n    }\n  ]\n}\n```"
}
```

### 2.5 Parsing the Output
In [`server/dronahq.py`](file:///home/Krishna-Singh/WarRoom/server/dronahq.py), WARROOM:
1. Inspects `data["response"]`.
2. Strips markdown backticks if present.
3. Deserializes JSON.
4. Reads the `"competitive_signals"` array and normalizes each entry into the standard `CompetitiveSignal` schema.

---

## 3. Nasiko Integration

### 3.1 Status
**Planned / Not Yet Implemented in Codebase.**

### 3.2 Context
Nasiko is designated as the multi-agent buildathon ecosystem partner for advanced agent orchestration. Future iterations may leverage Nasiko to coordinate specialized sub-agents (e.g., Pricing Scraper, Sales Battlecard Generator, Regulatory Alert Bot). 

Currently, no Nasiko endpoints, SDKs, or credentials exist in the codebase, and no mock endpoints have been invented.
