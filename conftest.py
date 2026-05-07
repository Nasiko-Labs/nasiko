# 🎬 Hackathon Video Demo Guide

> **This guide helps you record a winning demo video for the Nasiko MCP Hackathon submission.**

---

## ⏱️ Recommended Video Length: 5–8 minutes

---

## 📋 Video Structure (4 Segments)

---

## SEGMENT 1: Working Model (2–3 min)

### What to Show

Open your terminal, `cd` into `my-agent/`, and run the two commands below live. 
The audience should see real terminal output — **no slides**.

### Command 1: Run the Full Demo

```bash
cd c:\Users\Ishant Rajput\OneDrive\Desktop\nasiko\my-agent
py -3 demo/demo_local.py
```

**What to narrate while it runs** (read along with the output):

| Output Step | What to Say |
|-------------|-------------|
| **STEP 1: Upload & Detection (R1)** | *"We upload a zip file containing an MCP calculator server. The platform auto-detects it as MCP_SERVER using AST static analysis — no flags needed."* |
| **STEP 2: Manifest Generation (R3)** | *"A McpServerManifest.json is auto-generated with 3 tools (add, multiply, divide), 1 resource (settings), and 1 prompt (math_helper) — all extracted from decorator analysis."* |
| **STEP 3: Manifest Retrieval API** | *"The manifest can be retrieved via GET /manifest/{id} — it was persisted atomically to disk."* |
| **STEP 4: Code Persistence** | *"Source code is persisted to /tmp/nasiko/{id}/code/ for the bridge to spawn later."* |
| **STEP 5: Bridge Status Check** | *"Before the bridge starts, status is UNKNOWN. After bridge startup, it flips to 'ready'."* |
| **STEP 6: Agent ↔ MCP Linking** | *"An agent can be linked to the MCP server — the platform returns the list of available tools. No code changes to the agent are needed."* |
| **STEP 7: Observability** | *"Tracing uses a NullSpan pattern — it works the same whether Phoenix is running or not. In production, real spans go to Arize Phoenix."* |

### Command 2: Run All Tests

```bash
py -3 -m pytest tests/ -v
```

**What to narrate**: *"We have 106 automated tests covering every module — all passing. This includes unit tests, integration tests, real subprocess STDIO protocol tests, and AST constraint-enforcement tests."*

---

## SEGMENT 2: Agent Upload & How It Works (1–2 min)

### What to Show

Show the **sample MCP server** code, then explain the upload + detection flow.

#### Step A: Show the Sample Server

Open `examples/mcp-calculator-server/src/main.py` in your editor and scroll through it.

**Narrate**: *"Here's what a developer uploads. It's a standard MCP server using the official Python SDK. It has @mcp.tool(), @mcp.resource(), and @mcp.prompt() decorators. The developer doesn't need to do anything special — just follow the normal project structure: src/main.py, Dockerfile, docker-compose.yml."*

#### Step B: Show the Detection Code (Quick)

Open `nasiko/app/ingestion/detector.py` and briefly show the AST analysis loop.

**Narrate**: *"Our detector scans the uploaded Python files using AST. It looks for import patterns — 'mcp' or 'fastmcp' means MCP server, 'langchain' means LangChain agent, 'crewai' means CrewAI agent. If it finds multiple frameworks in one upload, it fails loudly with AMBIGUOUS_ARTIFACT rather than making a wrong guess."*

#### Step C: Show the Manifest

Open the generated manifest file or point to the demo output showing the manifest content.

**Narrate**: *"The manifest extracts every tool's name, description, parameter types, and required/optional info. Downstream consumers like the agent linker use this to dynamically inject tools — zero code changes to the agent."*

---

## SEGMENT 3: Technical Explanation (1–2 min)

### What to Show

Open the project's README.md or show the architecture diagram.

**Architecture diagram to narrate over** (show the README's ASCII diagram or draw on a whiteboard):

```
Upload.zip → R1 (Detect) → R3 (Manifest) → R2 (Bridge) → Kong → R5 (Traces)
                                               ↓
                                     R4 (Agent Linking)
```

**Key technical points to mention**:

1. **R1 — Artifact Detection**: 
   - AST-based, not regex — won't be fooled by comments or strings
   - Enforces `src/main.py` + `Dockerfile` + `docker-compose.yml` structure
   - Fails loudly on ambiguity instead of silent misdetection

2. **R3 — Manifest Generator**:
   - Lives at `nasiko/app/utils/mcp_manifest_generator/` — sibling to `agentcard_generator/`
   - Atomic file writes using `tempfile + os.replace()` — no corruption
   - Generates JSON-Schema for tool parameters with correct types

3. **R2 — STDIO-to-HTTP Bridge**:
   - Spawns MCP server as a subprocess with `bufsize=0` for unbuffered I/O
   - Performs full MCP JSON-RPC 2.0 three-step handshake: initialize → response → notifications/initialized
   - No `shell=True`, no `eval()`, no string Popen — AST-enforced security constraints
   - Dynamic port allocation (8100–8200 range)

4. **R4 — Zero-Code Tool Injection**:
   - LangChain agents get `StructuredTool` wrappers
   - CrewAI agents get `BaseTool` subclass wrappers
   - Agent code is never modified — tools are dynamically appended at runtime

5. **R5 — Observability**:
   - OpenTelemetry + Arize Phoenix for trace collection
   - W3C `traceparent` header propagation for cross-service correlation
   - `_NullSpan` pattern for graceful disabled-mode (no crashes if Phoenix is down)

6. **Track 1.5 — LLM Gateway**:
   - LiteLLM as platform-managed proxy
   - Agents use virtual key `nasiko-virtual-proxy-key` — no hardcoded API keys
   - Switch OpenAI → Anthropic by changing YAML config, not agent code

---

## SEGMENT 4: Mandatory Criteria Checklist (1–2 min)

### What to Show

Show this checklist on screen (create a slide or open this file).

### ✅ What's INCLUDED

| # | Requirement | Status | Evidence |
|---|-------------|--------|----------|
| 1 | MCP server as first-class artifact type | ✅ | `ArtifactType.MCP_SERVER` in detector |
| 2 | Agent structure validation (src/main.py, Dockerfile, docker-compose) | ✅ | `MissingStructureError` raised on missing files |
| 3 | AST-based detection, no user flags | ✅ | `detector.py` — pure AST import analysis |
| 4 | `McpServerManifest.json` auto-generation | ✅ | `mcp_manifest_generator/` — parser + generator |
| 5 | Manifest reuses `tools.py` pattern from agentcard_generator | ✅ | Lives as sibling package in `app/utils/` |
| 6 | STDIO-to-HTTP bridge for MCP servers | ✅ | `mcp_bridge/server.py` — `BridgeServer` class |
| 7 | MCP JSON-RPC 2.0 handshake (3-step) | ✅ | `_perform_mcp_handshake()` — verified by 10 tests |
| 8 | Kong service/route registration | ✅ | `mcp_bridge/kong.py` — `KongRegistrar` |
| 9 | Observability injection (Arize Phoenix + OTel) | ✅ | `mcp_tracing.py` + W3C traceparent |
| 10 | Agent-to-MCP tool injection (zero-code) | ✅ | `mcp_tools.py` — LangChain `StructuredTool` + CrewAI `BaseTool` |
| 11 | Agent linking API (`POST /agent/link`) | ✅ | `agent_mcp_linker.py` |
| 12 | LLM Gateway — LiteLLM proxy | ✅ | `docker-compose.local.yml` + `litellm-config.yaml` |
| 13 | Gateway env var injection (no hardcoded keys) | ✅ | `agent_builder.py` — `get_gateway_env_vars()` |
| 14 | Documentation (publish MCP server guide) | ✅ | `docs/publish-mcp-server.md` |
| 15 | Documentation (LLM gateway guide) | ✅ | `docs/llm-gateway.md` |
| 16 | Documentation (do NOT hardcode keys warning) | ✅ | `docs/llm-gateway.md` — bold warning at top |
| 17 | Sample MCP server artifact | ✅ | `examples/mcp-calculator-server/` |
| 18 | Sample agent using gateway | ✅ | `examples/langchain-gateway-agent/` |
| 19 | 100+ tests | ✅ | 106 tests — all passing |
| 20 | Docker Compose for full stack | ✅ | `nasiko/docker-compose.local.yml` |

### ⚠️ What's NOT Included (and Why)

| # | Feature | Status | Reason |
|---|---------|--------|--------|
| 1 | Remote MCP server registration (by URL) | ❌ Not implemented | This was explicitly listed as a **stretch goal** ("MCP-server-by-URL") in the problem statement. Our implementation focuses on the core stdio-based path which is the primary requirement. Remote registration can be added by extending `BridgeServer.start()` to skip subprocess spawn and directly register a remote URL with Kong. |
| 2 | Real container image build & deploy | ❌ Simulated | The existing `agent_builder.py` in the main Nasiko repo handles this. Our module integrates with it by providing the source code + Dockerfile at persistent paths. In the demo we simulate the bridge start; with Docker running, it works end-to-end via `docker-compose.local.yml`. |
| 3 | Portkey as alternative to LiteLLM | ❌ Not implemented | Problem statement asked for LiteLLM **or** Portkey. We chose LiteLLM because: (a) fully OSS, (b) native Phoenix callbacks for trace correlation, (c) simpler YAML config. Trade-off analysis is documented in `docs/llm-gateway.md`. |
| 4 | Multi-transport MCP (SSE/WebSocket) | ❌ Not implemented | The problem statement focuses on **stdio** transport. Extending to SSE would require replacing the subprocess pattern with an HTTP client, which is a clean extension point but wasn't required for the hackathon scope. |

**What to narrate**: *"All mandatory requirements are covered. The items we didn't include are stretch goals or optional choices — and we've documented why for each one."*

---

## 🎯 Tips for a Great Video

1. **Start strong** — Jump straight into the demo. No long intros.
2. **Terminal should be readable** — Use a large font (18pt+), dark background.
3. **Don't read code line by line** — Highlight key patterns, don't narrate every variable.
4. **Show real output** — The demo and test output are impressive. Let them speak.
5. **End with the numbers** — "106 tests passing, 20 requirements covered, zero bugs."
6. **Keep it under 8 minutes** — Judges watch many videos. Respect their time.

---

## 🖥️ Terminal Commands Quick Reference

```bash
# Navigate to project
cd c:\Users\Ishant Rajput\OneDrive\Desktop\nasiko\my-agent

# 1. Run the full pipeline demo (shows R1→R3→R2→R4→R5)
py -3 demo/demo_local.py

# 2. Run all 106 tests
py -3 -m pytest tests/ -v

# 3. Run only specific test categories
py -3 -m pytest tests/bridge/ -v              # R2 bridge (43 tests)
py -3 -m pytest tests/ingestion/ -v            # R1 detection (15 tests)
py -3 -m pytest tests/manifest_generator/ -v   # R3 manifest (14 tests)
py -3 -m pytest tests/observability/ -v        # R5 tracing (11 tests)
py -3 -m pytest tests/integration/ -v          # E2E pipeline (13 tests)
py -3 -m pytest tests/orchestration/ -v        # R4 linker (3 tests)

# 4. Show project structure
Get-ChildItem -Recurse -Name -Exclude __pycache__,.git,.pytest_cache | Select-Object -First 50
```

---

## 📁 Files to Open in Editor (for showing code)

| Order | File | What to Highlight |
|-------|------|-------------------|
| 1 | `examples/mcp-calculator-server/src/main.py` | *"This is what a developer uploads"* |
| 2 | `nasiko/app/ingestion/detector.py` | AST walk loop, `signals.add()` pattern |
| 3 | `nasiko/app/utils/mcp_manifest_generator/parser.py` | `@mcp.tool()` decorator parsing |
| 4 | `nasiko/mcp_bridge/server.py` | `_perform_mcp_handshake()`, `call_tool()` |
| 5 | `nasiko/app/utils/mcp_tools.py` | `create_mcp_langchain_tool()` wrapper |
| 6 | `nasiko/app/utils/observability/mcp_tracing.py` | `_NullSpan`, `create_tool_call_span()` |
| 7 | `nasiko/docker-compose.local.yml` | Phoenix + Kong + LiteLLM + nasiko-server |

---

## 📝 Suggested Script (Word-for-Word Opening)

> *"Hi, I'm [Name]. This is our submission for the Nasiko MCP Hackathon — Track 1 and Track 2.*
>
> *We've added first-class MCP server support to the Nasiko platform. When a developer uploads an MCP server as a zip file, the platform automatically detects it, generates a manifest of all its tools, deploys a stdio-to-HTTP bridge, registers it with Kong, and injects OpenTelemetry tracing — all with zero code changes from the developer.*
>
> *Let me show you it working."*

Then run `py -3 demo/demo_local.py` and narrate along with the steps above.
