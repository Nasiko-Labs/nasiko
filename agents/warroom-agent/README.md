# WARROOM

WARROOM is an AI Competitive Intelligence & Response Agent that continuously monitors public competitive movements, extracts factual evidence through live web research, executes post-retrieval reasoning against a company's specific product lineup and customer segments, and provides actionable responses for product, sales, and strategy teams without relying on fabricated claims or ungrounded statistics.

## Architecture

```text
External Client / Nasiko Control Plane
                 ↓ (POST /a2a: JSON-RPC SendMessage)
       FastAPI Service (WARROOM Agent)
       ├── GET /.well-known/agent-card.json (A2A Discovery)
       ├── POST /api/scan (Competitive Landscape Scanner)
       ├── POST /api/simulate (Response Simulator)
       └── POST /api/ask (Contextual Q&A)
                 ↓
       Anakin.io (Live Web Search & Sources)
                 ↓
       DronaHQ (AI Strategic Reasoning & Extraction)
```

## Platform Roles

- **Anakin.io**: Live web search engine gathering verified public sources, URLs, and snippets across competitors (Cashfree, Razorpay, PayU).
- **DronaHQ**: AI reasoning engine evaluating competitive moves against company product lines and customer segments, generating impact assessments, talk tracks, and investigation recommendations.
- **Nasiko / A2A Protocol**: Open Developer Control Plane integration via the Linux Foundation A2A v1.0 standard. Discovers agent capabilities via `AgentCard.json` (`/.well-known/agent-card.json`) and proxies `SendMessage` JSON-RPC tasks to `POST /a2a`.

## Core Endpoints

- `GET /api/health` — Service health check
- `POST /api/scan` — Live multi-competitor scan and rich signal extraction
- `POST /api/simulate` — Decision-support simulation across Product, Sales, Marketing, and Strategy
- `POST /api/ask` — Context-bounded Q&A strictly grounded in verified research evidence
- `GET /.well-known/agent-card.json` — A2A AgentCard discovery
- `POST /a2a` — A2A JSON-RPC 2.0 message dispatch (`SendMessage` / `message/send`)

## Setup & Running Locally

1. Create and activate a Python virtual environment:
   ```bash
   python3 -m venv .venv
   source .venv/bin/activate
   ```

2. Install dependencies:
   ```bash
   pip install -r requirements.txt
   ```

3. Configure environment variables (`.env`):
   ```bash
   cp .env.example .env
   # Fill in ANAKIN_API_KEY, DRONAHQ_WEBHOOK_URL, DRONAHQ_API_KEY
   ```

4. Run the backend server:
   ```bash
   uvicorn server.main:app --host 127.0.0.1 --port 8000
   ```

5. Or run via Docker Compose:
   ```bash
   docker compose up --build
   ```

## Running Automated Tests

Run the complete test suite (43 unit tests):
```bash
source .venv/bin/activate && pytest -v
```

