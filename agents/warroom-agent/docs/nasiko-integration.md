# Nasiko Integration Architecture

> **Phase 7: A2A-Compliant Agent Ingress & Developer Control Plane Compatibility**

---

## 1. Overview & Role of Nasiko

**Nasiko** ([`Nasiko-Labs/nasiko`](https://github.com/Nasiko-Labs/nasiko)) is an open-source Developer Control Plane for AI agents built in Rust. It serves as an operational proxy and governance gateway for fleets of containerized AI agents communicating over the open **A2A (Agent-to-Agent)** protocol (Linux Foundation).

### How WARROOM Integrates with Nasiko:
- **Opaque Autonomous Peer**: WARROOM integrates as a standalone, containerized A2A agent.
- **Independent Internal Intelligence**: WARROOM internally orchestrates live web research (Anakin.io) and strategic reasoning (DronaHQ). The Nasiko control plane does not need to know or manage WARROOM's internal prompt chains or research pipelines.
- **Discovery**: Nasiko discovers WARROOM's identity, version, and capabilities by fetching its `AgentCard.json` via standard well-known endpoints.
- **Invocation**: When external clients or peer agents send tasks to WARROOM, Nasiko terminates TLS, enforces access controls, records OpenTelemetry traces, and forwards JSON-RPC `SendMessage` requests to WARROOM's `/a2a` endpoint.

```
External User / Control Plane Client
                 ↓
      Nasiko Control Plane (nasiko-server)
      - Ingress TLS Termination
      - Auth & Rate Limiting
      - OpenTelemetry Tracing
      - A2A Agent Proxy (/api/agents/{id}/*)
                 ↓ (POST /a2a: JSON-RPC SendMessage)
      WARROOM Agent Container (Port 8000)
      ├── GET /.well-known/agent-card.json (Discovery)
      └── POST /a2a (A2A 1.0 JSON-RPC 2.0 Ingress)
                 ↓
      WARROOM Pipeline
      ├── Anakin.io (Live Web Search & Sources)
      └── DronaHQ (Strategic Competitive Reasoning)
```

---

## 2. Endpoints & Protocol Specification

### 2.1 Discovery Endpoints
- **Primary**: `GET /.well-known/agent-card.json` (A2A 1.0 standard)
- **Legacy Alias**: `GET /.well-known/agent.json` (A2A 0.3 fallback)

Returns the canonical `AgentCard.json` declaring:
- `name`: `"warroom-agent"`
- `version`: `"1.0.0"`
- `protocolVersion`: `"1.0"`
- `preferredTransport`: `"JSONRPC"`
- `skills`:
  1. `competitive-scan`: Live multi-competitor research & signal extraction.
  2. `response-simulate`: Multi-departmental response option simulation.
  3. `ask-warroom`: Context-bounded natural language competitive Q&A.

### 2.2 Runtime Ingress Endpoint
- **Path**: `POST /a2a`
- **Protocol**: JSON-RPC 2.0 over HTTP
- **Optional Header**: `A2A-Version: 1.0`
- **Supported Methods**:
  - `SendMessage` (A2A 1.0 standard)
  - `message/send` (A2A 0.3 dialect)

---

## 3. A2A Wire Protocol Examples

### 3.1 Request (`SendMessage`)
```json
{
  "jsonrpc": "2.0",
  "id": "demo-001",
  "method": "SendMessage",
  "params": {
    "message": {
      "messageId": "msg-001",
      "role": "ROLE_USER",
      "parts": [
        {
          "text": "Why does Cashfree RiskShield matter to PayFlow?"
        }
      ]
    },
    "configuration": {
      "acceptedOutputModes": ["text/plain"]
    }
  }
}
```

### 3.2 Response (`SendMessageResponse`)
```json
{
  "jsonrpc": "2.0",
  "id": "demo-001",
  "result": {
    "task": {
      "id": "task-2d15db7b50ff",
      "contextId": "ctx-fcd576e40725",
      "status": {
        "state": "TASK_STATE_COMPLETED",
        "timestamp": "2026-09-20T09:33:11.078856+00:00"
      },
      "artifacts": [
        {
          "artifactId": "art-64175a86a442",
          "parts": [
            {
              "text": "Cashfree launched 'RiskShield', a real-time risk management solution aimed at reducing fraudulent activities using AI and ML...\n\nKey Takeaways:\n- Competitor: Cashfree\n- Significance: SIGNIFICANT\n- Affected Products: FraudShield\n\nVerified Evidence Sources:\n- https://www.cashfree.com/news-room/cashfree-payments-launches-riskshield...\n\nConfidence: medium"
            }
          ]
        }
      ]
    }
  }
}
```

---

## 4. Packaging & Deployment Manifests

The repository provides the exact manifests expected by Nasiko's deployment engine (`cli/src/commands/validate.rs` and `deploy.rs`):
- **`AgentCard.json`**: Root identity file validated by `nasiko card` and `nasiko validate`.
- **`Dockerfile`**: Container runtime recipe based on `python:3.11-slim`, running `uvicorn server.main:app` on port 8000.
- **`docker-compose.yml`**: Local multi-container orchestration definition with healthcheck configured on `/api/health`.

---

## 5. Local Verification & Limitations

### Verified Locally:
- Full automated test suite passes: **43 / 43 tests green** (`pytest -v`).
- `AgentCard.json` validated against all 8 mandatory fields from Nasiko `validate.rs`.
- `GET /.well-known/agent-card.json` verified live on port 8000.
- `POST /a2a` with `SendMessage` verified live against running server with synchronous DronaHQ reasoning.
- `POST /a2a` with legacy `message/send` verified live.
- Existing REST endpoints (`/api/health`, `/api/scan`, `/api/simulate`, `/api/ask`) verified completely unaffected.

### Limitations:
- No live Nasiko control-plane cluster deployment was performed. Local verification tested the agent container and A2A wire contract directly.
- The React frontend remains intentionally frozen in accordance with the backend-first architecture roadmap.
