# Nasiko Router Lab — P2 Request Classifier

Interactive demonstration console and routing instrument for **Nasiko's P2 Request Classifier**. The web application allows users to enter prompts (and optional context), evaluates query intent and complexity, and demonstrates how requests are mapped to optimal AI model tiers (Tier 1–3).

The frontend is designed for deployment on **Vercel**, communicating securely via a Next.js App Router server route (`/api/classify`) to the classifier service hosted on **Railway**.

---

## Architecture & Flow

```text
[ Browser Client ]
        │  POST /api/classify (JSON: { query, context })
        ▼
[ Next.js API Route (Vercel Serverless) ]
        │  Reads server-only RAILWAY_API_URL & RAILWAY_API_TOKEN
        │  Validates input, manages timeouts, normalizes response
        ▼
[ Classifier Backend (Railway) ]
        │  Returns { request_type, complexity, confidence, tier, ... }
        ▼
[ Normalized Telemetry Display ]
   • Request Type (e.g., Code Generation, Technical Design)
   • Complexity Score (1–5 scale)
   • Confidence Percentage (0–100%)
   • Selected Model Tier (Tier 1, 2, or 3)
   • Fallback Status Indicator
   • Optional Baseline Benchmark Comparison
```

---

## Page Structure

1. **Home (`/`)**:
   - Technical landing page introducing Nasiko's adaptive routing model.
   - Retains the iconic WebGL human illustration (`OMzqyUv6M3kSnv0JeAtC`) exclusively on this page.
   - Includes **"Open Classifier"** direct navigation and a **"How It Works"** architectural breakdown section.
2. **Classifier Console (`/classifier`)**:
   - Working P2 demonstration instrument.
   - Shares the ambient dot/matrix particle atmosphere without the human illustration.
   - Interactive prompt submission, conversation context, 4 sample evaluation buttons, tier selection, complexity scoring, and baseline comparisons.

---

## Local Development Setup

### 1. Install Dependencies
```bash
npm install
```

### 2. Configure Environment Variables
Copy `.env.example` to `.env.local`:
```bash
cp .env.example .env.local
```

Edit `.env.local` with your Railway deployment URL and optional token:
```env
RAILWAY_API_URL=https://your-classifier-app.railway.app/classify
RAILWAY_API_TOKEN=your_optional_railway_token_here
```

> **Security Note:** Never prefix backend credentials with `NEXT_PUBLIC_`. Environment variables without `NEXT_PUBLIC_` are strictly kept server-side inside Next.js and are never exposed in the browser bundle.

### 3. Run Development Server
```bash
npm run dev
```

Open [http://localhost:3000](http://localhost:3000) in your browser.

---

## Vercel Deployment

1. Push your repository to GitHub.
2. Import the project into your [Vercel Dashboard](https://vercel.com).
3. In **Settings → Environment Variables**, add:
   - `RAILWAY_API_URL`: The full URL to your Railway classifier endpoint (e.g., `https://nasiko-classifier.up.railway.app/classify`).
   - `RAILWAY_API_TOKEN`: (Optional) Bearer token if your Railway endpoint requires authentication.
4. Deploy.

---

## API Contract Specification

### Request Payload (`POST /api/classify` and Railway endpoint)
```json
{
  "query": "Write a Python script that parses CSV files into Parquet",
  "context": "Optional system prompt, conversation history, or environment metadata"
}
```

### Response Payload (From Railway Classifier)
The server route normalizes both `snake_case` and `camelCase` representations. Expected fields:
```json
{
  "request_type": "code_generation",
  "complexity": 3,
  "confidence": 0.94,
  "tier": "tier_2",
  "classifier": "adaptive-router-v2",
  "fallback_used": false,
  "latency_ms": 28,
  "baseline": {
    "request_type": "general",
    "complexity": 3,
    "confidence": 0.5,
    "tier": "tier_2",
    "classifier": "regex-baseline",
    "fallback_used": true,
    "latency_ms": 2
  }
}
```

#### Field Explanations
| Field | Type | Description |
| :--- | :--- | :--- |
| `request_type` | `string` | Categorical intent (`code_generation`, `code_understanding`, `technical_design`, `analytical_reasoning`, `writing`, `factual_lookup`, `general`). |
| `complexity` | `integer (1–5)` | Computational depth and reasoning requirement. |
| `confidence` | `float (0.0–1.0)` | Model's confidence probability for the predicted category. |
| `tier` | `string` | Selected strength tier: `tier_1` (frontier/complex), `tier_2` (balanced), `tier_3` (fast/simple). |
| `classifier` | `string` *(optional)* | Identifier of the underlying classifier model (e.g. `fasttext`, `distilbert`, `bandit`). |
| `fallback_used` | `boolean` *(optional)* | Flag indicating whether safe fallback rules were triggered. |
| `latency_ms` | `number` *(optional)* | Inference latency in milliseconds (or `latency_us` in microseconds). |
| `baseline` | `object` *(optional)* | Baseline benchmark or comparison result for A/B evaluation. |

---

## Checklist for Backend Developers

Before connecting the production Railway backend, please coordinate the following details:
1. **Endpoint Path**: Confirm whether the POST endpoint path is `/classify`, `/v1/classify`, or a root `/`. (Adjust `RAILWAY_API_URL` accordingly).
2. **Authentication**: Confirm if requests require a `Bearer` token or custom headers. If custom headers are required, configure them in [`src/app/api/classify/route.ts`](src/app/api/classify/route.ts).
3. **Response Schema**: If your classifier output uses different field names, update the normalizer mapping in [`src/lib/classifier.ts`](src/lib/classifier.ts).

---

## Demo Mode

If the Railway backend is offline or undergoing deployment, the console features an explicit **DEMO MODE** switch in the top header. When enabled:
- The UI is clearly labelled with a prominent **`[DEMO DATA — NOT LIVE]`** badge.
- Inputs are evaluated against deterministic local heuristic patterns to showcase UI states, complexity meters, confidence gauges, and tier badges without making live network calls.
- Demo mode is **disabled by default**.
