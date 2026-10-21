# Architecture Decision Records (ADRs)

> **Key technical and design decisions, context, trade-offs, and rationale.**

---

### ADR-001: FastAPI Monolith for MVP
- **Status**: Accepted
- **Context**: The hackathon project requires an API gateway to coordinate web search, evidence gathering, LLM agent reasoning, and client dashboard delivery.
- **Decision**: Implement the backend as a single FastAPI monolithic process rather than multiple microservices, background job workers, or serverless functions.
- **Rationale**:
  - Eliminates distributed network failures and inter-service synchronization latency.
  - Zero external infrastructure dependencies (no Redis, Celery, RabbitMQ, or PostgreSQL).
  - Rapid local setup, trivial containerization, and easy debugging in under 5 minutes.

---

### ADR-002: Anakin Synchronous Search API for Research
- **Status**: Accepted
- **Context**: Anakin.io provides both a synchronous `/v1/search` endpoint and an asynchronous `/v1/agentic-search` endpoint.
- **Decision**: Utilize `POST https://api.anakin.io/v1/search` with a default limit of 3 results per competitor as the primary research engine.
- **Rationale**:
  - `/v1/search` returns verified search results (titles, URLs, snippets) in ~1.2 seconds.
  - `/v1/agentic-search` takes several minutes, requires webhook callbacks or long-polling, and introduces significant latency risk for live dashboard demo sessions.
  - The collected snippets from `/v1/search` provide more than enough factual evidence for downstream LLM reasoning.

---

### ADR-003: DronaHQ as the Reasoning Engine
- **Status**: Accepted
- **Context**: Competitive signal analysis requires sophisticated business reasoning (detecting feature overlap, assessing threat level, generating sales battlecards) rather than simple text matching.
- **Decision**: Offload the strategic reasoning prompt to a published DronaHQ AI agent via its configured webhook URL.
- **Rationale**:
  - Decouples prompt engineering and LLM orchestration from the FastAPI application.
  - Leverages DronaHQ's agentic workflow environment and structured JSON schema enforcement.
  - Enables rapid iteration on the agent's system prompts in DronaHQ without redeploying the backend.

---

### ADR-004: Preserve Raw Web Evidence in Response
- **Status**: Accepted
- **Context**: When LLMs generate summaries, users often want to verify the claims against the underlying source material.
- **Decision**: Return the complete array of 9 raw Anakin search results (`all_research: list[CompetitorResearch]`) alongside the synthesized signals in `ScanResponse`.
- **Rationale**:
  - Complete transparency: users can click direct URLs to verify competitor blog posts or news releases.
  - Resilience: if DronaHQ reasoning encounters an error or returns `insufficient_evidence`, the raw research is still delivered to the client, providing value even during partial system degradation.

---

### ADR-005: Backend-Side Signal Normalization
- **Status**: Accepted
- **Context**: AI agent outputs frequently fluctuate across runs: wrapping output in markdown code fences (```` ```json ... ``` ````), alternating between snake_case (`competitive_signals`) and camelCase (`competitorSignals`), or returning strings instead of nested dicts.
- **Decision**: Implement comprehensive normalization in [`server/dronahq.py`](file:///home/Krishna-Singh/WarRoom/server/dronahq.py) before serializing data to the frontend.
- **Rationale**:
  - The client dashboard remains simple and decoupled from LLM prompt formatting changes.
  - The backend guarantees strong typing and validation via Pydantic (`CompetitiveSignal`).
  - Allows seamless mapping of competitor source URLs to each normalized signal.

---

### ADR-006: Direct Standard Webhook Consumption (No Polling)
- **Status**: Accepted
- **Context**: Early iterations of the DronaHQ webhook returned asynchronous acknowledgements (`status: pending`) with `thread_id` and `run_id`.
- **Decision**: Configure the DronaHQ webhook in **Standard** response mode and consume the execution output synchronously from the `"response"` field.
- **Rationale**:
  - DronaHQ Standard mode completes execution within the HTTP request lifecycle (~14–18s) and returns HTTP 200 with the full result.
  - Eliminates the need to construct polling endpoints, WebSocket connections, or background state tracking.
  - Rejects speculative endpoints (e.g. `/api/v1/agent/*`) that do not exist on DronaHQ's server.

---

### ADR-007: Anti-Hallucination on Insufficient Evidence
- **Status**: Accepted
- **Context**: LLMs often invent plausible-sounding competitor moves (e.g., fake price drops or fictitious feature releases) when given limited context.
- **Decision**: If DronaHQ returns `status: "insufficient_evidence"`, WARROOM returns an empty signal array (`signals = []`) and explicitly flags the status in `reasoning.status`.
- **Rationale**:
  - Product and strategy teams cannot tolerate fabricated intelligence.
  - No synthetic fallback data is injected when live research yields no actionable signals.

---

### ADR-008: UI Freeze During Backend Stabilization
- **Status**: Accepted
- **Context**: Modifying UI components while core integration contracts (Anakin payloads, DronaHQ wrappers, signal normalization) are actively evolving leads to rework and synchronization errors.
- **Decision**: Freeze the React frontend (`client/`) during Phase 3 backend stabilization.
- **Rationale**:
  - Keeps focus on end-to-end reliability, test coverage, and schema fidelity.
  - Once backend contracts are locked and documented, the frontend can be unfrozen and updated in a single clean pass.

---

### ADR-009: Rich Competitive Intelligence Model with Safe Fallbacks
- **Status**: Accepted
- **Context**: Competitive intelligence requires answering "Why it matters", "What internal products overlap", "Which teams are impacted", and "What should be investigated", rather than just "What changed".
- **Decision**: Extend `CompetitiveSignal` with `significance`, `overlap` (`SignalOverlap`), `impact` (`DepartmentImpact`), `recommended_actions`, and `confidence`, backed by flexible multi-key normalization in [`server/dronahq.py`](file:///home/Krishna-Singh/WarRoom/server/dronahq.py) that defaults safely when fields are omitted.
- **Rationale**:
  - Consumes rich strategic outputs already returned by the DronaHQ reasoning agent without additional LLM calls.
  - Preserves 100% backward compatibility with minimal/legacy signal responses.
  - Clear distinction between factual web evidence (sources, competitor, change) and strategic interpretations (significance, impact, recommendations).
