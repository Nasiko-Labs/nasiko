# WARROOM Documentation

> **From competitive signals to strategic action.**

WARROOM is an AI-powered competitive intelligence and response agent. It monitors public competitive developments through live web research, executes post-retrieval reasoning against a company's specific product lineup and target customer segments, and extracts evidence-backed competitive signals with impact assessments and recommended strategic actions—without fabricating claims or ungrounded statistics.

---

## Current Status

- **Phase**: Phase 3B Complete (Backend Pipeline & End-to-End Live Integration Verified)
- **Automated Tests**: 11/11 tests passing (`pytest -v`)
- **Live Search**: Verified via Anakin Search API (`POST https://api.anakin.io/v1/search`) across Cashfree, Razorpay, and PayU (9 live sources collected per scan).
- **Live Reasoning**: Verified via DronaHQ Standard Webhook (`POST <DRONAHQ_WEBHOOK_URL>`) returning structured execution outputs consumed directly by FastAPI.
- **Signal Normalization**: Operational in FastAPI, supporting markdown-fenced and JSON structured signals mapped to the backend `CompetitiveSignal` model.
- **Frontend**: Functional dashboard in `client/` (temporarily frozen during backend stabilization).

---

## Quick Architecture

```text
[React Dashboard (client/)]
           │
           │ HTTP POST /api/scan
           ▼
[FastAPI Backend (server/main.py)]
     │                           │
     │ 1. Web Search Queries     │ 2. Grounded Reasoning Payload
     ▼                           ▼
[Anakin Search API]       [DronaHQ Reasoning Agent]
(POST /v1/search)         (POST Webhook URL)
     │                           │
     ▼                           ▼
9 Live Source Results     Structured Execution Output
     │                           │
     └───────────┬───────────────┘
                 │
                 ▼
      [Signal Normalization & Evidence Linking]
                 │
                 ▼
         [ScanResponse JSON]
```

---

## Documentation Index

| Document | Description |
| :--- | :--- |
| [Architecture](file:///home/Krishna-Singh/WarRoom/docs/architecture.md) | Technical architecture, layer responsibilities, repository layout, and system design |
| [Product Concept](file:///home/Krishna-Singh/WarRoom/docs/product.md) | Problem statement, WARROOM core concept (Observe-Understand-Assess-Respond), PayFlow demo profile |
| [Data Flow](file:///home/Krishna-Singh/WarRoom/docs/data-flow.md) | Step-by-step trace of `/api/scan` request lifecycle, boundary payloads, and sequence |
| [Business Logic](file:///home/Krishna-Singh/WarRoom/docs/business-logic.md) | Research query generation, evidence grounding, signal parsing, normalization, and error resilience |
| [Integrations](file:///home/Krishna-Singh/WarRoom/docs/integrations.md) | API specifications and contracts for Anakin.io, DronaHQ, and Nasiko status |
| [API Reference](file:///home/Krishna-Singh/WarRoom/docs/api.md) | Endpoints (`/api/health`, `/api/company`, `/api/scan`), schemas, status codes, and curl examples |
| [Configuration](file:///home/Krishna-Singh/WarRoom/docs/configuration.md) | Environment variables, `.env.example`, settings loader, and secret management |
| [Testing Strategy](file:///home/Krishna-Singh/WarRoom/docs/testing.md) | Unit test coverage, MockTransport fixtures, test commands, and live verification |
| [Architecture Decisions (ADR)](file:///home/Krishna-Singh/WarRoom/docs/decisions.md) | Key architectural decisions, rationale, alternatives considered, and trade-offs |
| [Security](file:///home/Krishna-Singh/WarRoom/docs/security.md) | Secrets handling, server-side API mediation, CORS, and limitations |
| [Failure Modes](file:///home/Krishna-Singh/WarRoom/docs/failure-modes.md) | Failure matrix covering outages, malformed responses, timeouts, and mitigations |
| [Roadmap](file:///home/Krishna-Singh/WarRoom/docs/roadmap.md) | Implemented capabilities, immediate next steps, and stretch goals |
| [Diagrams](file:///home/Krishna-Singh/WarRoom/docs/diagrams/system-architecture.md) | Mermaid visual diagrams for system architecture, sequence, and signal lifecycle |

---

## Local Development Quickstart

### 1. Environment Setup

```bash
# Clone or navigate to the repository
cd ~/WarRoom

# Create and activate Python virtual environment
python3 -m venv .venv
source .venv/bin/activate

# Install dependencies
pip install -r requirements.txt
```

### 2. Configure Credentials

Copy the example environment file:
```bash
cp .env.example .env
```

Ensure `.env` contains your actual API credentials (never commit `.env`):
```ini
ANAKIN_API_KEY=your_anakin_key_here
DRONAHQ_WEBHOOK_URL=https://agents-backend.dronahq.com/webhook/...
DRONAHQ_API_KEY=your_dronahq_key_here
ANAKIN_LIMIT=3
PORT=8000
```

### 3. Run Automated Tests

```bash
source .venv/bin/activate
pytest -v
```

All 11 unit tests should pass.

### 4. Start the Backend Server

```bash
source .venv/bin/activate
uvicorn server.main:app --reload --port 8000
```

Verify backend health:
```bash
curl -s http://127.0.0.1:8000/api/health
# Output: {"status":"ok","service":"warroom-api"}
```

### 5. Trigger a Live Competitive Scan

```bash
curl -s -X POST http://127.0.0.1:8000/api/scan \
  -H "Content-Type: application/json" \
  -d '{}' | python3 -m json.tool
```
