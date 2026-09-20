# Product Roadmap & Milestones

> **Development status, upcoming implementation phases, and long-term vision.**

---

## 1. Implemented Capabilities (Phase 1 – Phase 4)

The following capabilities are fully implemented, verified, and backed by automated tests:

- [x] **FastAPI Gateway Core**:
  - `GET /api/health` liveness endpoint.
  - `POST /api/company` profile validation endpoint.
  - `POST /api/scan` full pipeline orchestration endpoint.
- [x] **Live Web Research Integration (Anakin.io)**:
  - Synchronous querying via `POST https://api.anakin.io/v1/search`.
  - Context-aware keyword prompt synthesis (`{competitor} {products} {segments}`).
  - Structured result extraction (titles, URLs, snippets, dates) across Cashfree, Razorpay, and PayU.
  - 9 live sources gathered per standard scan.
- [x] **AI Reasoning Integration (DronaHQ)**:
  - Dispatches structured research payload to published DronaHQ agent webhook.
  - Consumes **Standard** response mode synchronously from the `"response"` field.
  - Resilient parsing handling JSON strings, markdown fences (```` ```json ````), and pre-parsed objects.
- [x] **Rich Competitive Intelligence Model (Phase 4)**:
  - Extended `CompetitiveSignal` schema with `significance`, `overlap` (`SignalOverlap`), `impact` (`DepartmentImpact`), `recommended_actions`, and `confidence`.
  - Normalization extracts departmental ratings (Product, Sales, Marketing, Strategy), internal product overlaps, customer segments, investigation recommendations, and sales talk tracks without extra LLM round-trips.
  - Strict preservation of raw Anakin web evidence and zero fabrication on missing fields.
- [x] **Evidence Transparency & Non-Destructive Failure**:
  - Direct linking of Anakin source URLs to normalized signals.
  - Strict anti-hallucination handling on `insufficient_evidence`.
  - Full research data preservation if DronaHQ reasoning encounters a timeout or error.
- [x] **Automated Test Coverage**:
  - 13/13 unit tests passing (`pytest -v`) covering mock transports, rich signals, defaults, response variants, error states, and health checks.
- [x] **Demo Benchmark Context**:
  - Benchmark profile for `PayFlow` vs `Cashfree`, `Razorpay`, and `PayU` configured in `data/company_context.json`.
- [x] **Response Simulator Engine (Phase 5)**:
  - `POST /api/simulate` endpoint evaluating a `CompetitiveSignal`.
  - Structured decision-support options across Product, Sales, Marketing, and Strategy.
  - Integration with DronaHQ reasoning agent with strict anti-fabrication prompt instructions.
  - Evidence preservation with strict retention of `source_urls` and low confidence handling for signals lacking sources.
  - Non-destructive heuristic fallback when DronaHQ encounters timeouts, 5xx errors, or unparseable output.
  - 21/21 unit tests passing.
- [x] **Ask WARROOM Contextual Q&A Engine (Phase 6)**:
  - `POST /api/ask` endpoint answering questions over active `CompetitiveSignal` context.
  - Grounded in PayFlow company profile (Payment Gateway, Smart Routing, FraudShield, Global Payouts) and customer segments (SMB, Mid-Market).
  - Strict evidence citation integrity: validates output evidence URLs strictly against supplied signals' `source_urls`. Zero hallucinated URLs.
  - Immediate, structured insufficient context response when signals list is empty (0 external calls).
  - Heuristic fallback engine on upstream reasoning timeouts, HTTP failures, or malformed responses.
  - Full test suite: 32/32 tests passing (`pytest -v`).
- [x] **Nasiko Integration & A2A Ingress (Phase 7)**:
  - `AgentCard.json` conforming to Nasiko `validate.rs` schema with 3 declared skills (`competitive-scan`, `response-simulate`, `ask-warroom`).
  - Standard discovery at `GET /.well-known/agent-card.json` and `GET /.well-known/agent.json`.
  - A2A JSON-RPC 2.0 dispatch at `POST /a2a` supporting `SendMessage` and `message/send`.
  - Minimal container runtime (`Dockerfile`) and local orchestration (`docker-compose.yml`).
  - Full test suite: 43/43 tests passing (`pytest -v`).

---

## 2. Immediate Next Steps (Phase 8: Frontend Redesign & Presentation)

With the complete backend intelligence, simulation, conversational Q&A, and Nasiko A2A pipelines verified end-to-end, the immediate next focus is the frontend dashboard:

- [ ] **Unfreeze React Frontend (`client/`)**:
  - Reconnect the dashboard to the updated rich `/api/scan`, `/api/simulate`, and `/api/ask` response contracts.
  - Update TypeScript interfaces in `client/src/types.ts` to reflect `SignalOverlap`, `DepartmentImpact`, `ResponseSimulation`, and `AskWarroomResponse`.
- [ ] **Rich Signal Cards & Action Drawer**:

  - Render competitor tags, category badges, headlines, and significance badges (`CRITICAL`, `SIGNIFICANT`, `INFORMATIONAL`).
  - Render affected internal product overlap chips (`FraudShield`, `Payment Gateway`).
  - Render departmental impact indicators (Product, Sales, Marketing, Strategy).
  - Display actionable sales battlecards and product investigation recommendations.
  - Display direct source evidence chips linking out to original competitor blog posts and docs.
- [ ] **Interactive Response Simulator Drawer**:
  - Allow users to click "Simulate Response" on any signal card.
  - Trigger `POST /api/simulate` and display structured tabs/sections for Product, Sales, Marketing, and Strategy responses.
- [ ] **Ask WARROOM Contextual Chat Drawer**:
  - Grounded question-answering input bar with suggested starter questions.
  - Display answer, key takeaways, cited signals, and verified evidence links.


---

## 3. Stretch Goals & Future Vision (Post-MVP)

The following advanced capabilities are planned for future major releases:

### 3.1 Continuous Monitoring & Anakin Webhooks
- Leverage Anakin's scheduled website monitoring and webhook change alerts to detect competitor page changes automatically without requiring manual scan triggers.

### 3.2 Multi-Agent Orchestration via Nasiko
- Coordinate specialized multi-agent sub-teams (e.g. Pricing Extractor, Feature Matrix Comparator, Sales Objection Generator) using the Nasiko agent framework.

### 3.3 Historical Trend Timelines & Persistent Storage
- Introduce database persistence (PostgreSQL / SQLite) to track competitive movements over weeks and months, generating longitudinal threat trends and quarterly summaries.

### 3.4 Automated Team Alerting Channels
- Push notifications to Slack channels, Microsoft Teams, or automated weekly executive email digests.

