# PLAN-grantmatch-setup

## 1. Context & Scope
**Project:** GrantMatch AI Setup (Buildathon Project)
**Objective:** Set up the GrantMatch AI project utilizing local host execution for Python agents, local Nasiko orchestration (via Docker), and DronaHQ with Ngrok for tunneling.
**Working Directory:** `d:\Projects\GrantMach AI`

## 2. Task Breakdown

### Phase 1: Environment & API Preparation
- [x] Create `agents/` directory structure in the current workspace.
- [x] Initialize Python virtual environment (`.venv`) and install dependencies (`fastapi`, `uvicorn`, `httpx`, `groq`).
- [x] Set up `.env` file in the workspace with `GROQ_API_KEY` and `ANAKIN_API_KEY`.
- [ ] Verify Nasiko orchestrator is running properly via Docker at `localhost:8080`.

### Phase 2: Agent Implementation (Local Host Execution)
- [x] **Agent 1:** Create `agents/grant_scout/main.py` (Searches live databases for grants).
- [x] **Agent 2:** Create `agents/compliance_analyst/main.py` (Analyzes alignment and compliance).
- [x] **Agent 3:** Create `agents/synthesis/main.py` (Synthesizes final brief using Groq).
- [x] Create a script to run all 3 FastAPI services on different ports (e.g., 8001, 8002, 8003).

### Phase 3: Networking & Tunneling
- [ ] Install/Configure Ngrok to expose local agent APIs or the Nasiko orchestrator to the public web.
- [ ] Document the public Ngrok URLs for DronaHQ configuration.
- [ ] Connect Nasiko orchestrator with the local Python agents.

### Phase 4: UI & Integration (DronaHQ)
- [ ] Define the payload structure expected by DronaHQ.
- [ ] Connect DronaHQ POST requests to the Ngrok URL pointing to the Nasiko endpoint.
- [ ] Map the UI components (Input Panel, Status Panel, Report Panel) to the API response fields.

## 3. Agent Assignments
- **project-planner:** Responsible for this plan.
- **backend-specialist:** Will handle the Python FastAPI agent implementations, Nasiko routing, and Ngrok tunneling.
- **frontend-specialist:** (Optional) Will assist with DronaHQ API connection configurations.

## 4. Verification Checklist
- [ ] Ensure all 3 Python agents respond successfully to `/health` endpoints.
- [ ] Validate Anakin.io integration returns markdown content for grant searches.
- [ ] Confirm Groq LLM parses and returns the expected JSON objects.
- [ ] Verify DronaHQ can successfully trigger the pipeline via the Ngrok tunnel and display the synthesis report.
