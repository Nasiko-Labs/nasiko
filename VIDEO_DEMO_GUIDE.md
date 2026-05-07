# Nasiko MCP Server Publishing & LLM Gateway

> **Track 1 + Track 2** submission for the Nasiko MCP Hackathon

First-class MCP server support and a platform-managed LLM gateway for the
[Nasiko](https://github.com/Nasiko-Labs/nasiko) AI-agent registry and
orchestration platform.

---

## What This Adds

### Track 1: MCP Server Publishing & Agent Integration

| Capability | Description |
|------------|-------------|
| **Artifact Detection** | AST-based auto-detection of MCP servers from `mcp`/`fastmcp` imports — no flags needed |
| **Structure Validation** | Enforces `src/main.py` + `Dockerfile` + `docker-compose.yml` contract |
| **Manifest Generation** | Auto-generates `McpServerManifest.json` with all tools, resources, and prompts |
| **STDIO-to-HTTP Bridge** | Spawns MCP server as subprocess, performs JSON-RPC 2.0 handshake, exposes HTTP API |
| **Kong Routing** | Registers MCP servers with Kong for discoverability at `/mcp/{id}/` |
| **Observability** | OpenTelemetry + Arize Phoenix tracing on every tool call with W3C traceparent propagation |
| **Agent-to-MCP Wiring** | Zero-code tool injection for LangChain and CrewAI agents |
| **Ambiguity Detection** | Fails loudly on mixed-framework uploads instead of silent misdetection |

### Track 2: LLM Gateway

| Capability | Description |
|------------|-------------|
| **LiteLLM Proxy** | Platform-managed LLM gateway — agents use a virtual key, no hardcoded provider keys |
| **Provider Switching** | Change OpenAI → Anthropic by editing gateway config, not agent code |
| **Auto-Injection** | Gateway URL + virtual key injected into agent environment at deploy time |
| **Trace Correlation** | Gateway requests traced and correlated with agent spans in Phoenix |

---

## Architecture

```
┌──────────────────────────────────────────────────────────┐
│                    Nasiko Platform                        │
│                                                          │
│  ┌─────────┐   ┌─────────┐   ┌─────────┐   ┌────────┐ │
│  │   R1    │──>│   R3    │──>│   R2    │──>│  Kong  │ │
│  │ Ingest  │   │Manifest │   │ Bridge  │   │Gateway │ │
│  │& Detect │   │ Gen     │   │ Server  │   │        │ │
│  └─────────┘   └─────────┘   └────┬────┘   └────────┘ │
│                                    │                     │
│                    ┌───────────────┼───────────────┐     │
│                    │               │               │     │
│               ┌────▼────┐   ┌─────▼─────┐  ┌──────▼──┐ │
│               │   R4    │   │    R5     │  │ LLM    │ │
│               │ Agent   │   │ Phoenix   │  │ Gateway│ │
│               │ Linker  │   │ Tracing   │  │LiteLLM│ │
│               └─────────┘   └───────────┘  └────────┘ │
└──────────────────────────────────────────────────────────┘
```

**Pipeline flow**: Upload zip → R1 detects artifact type → R3 generates manifest → R2 starts bridge → Kong routes → R5 traces → R4 links to agents

---

## Project Structure

```
my-agent/
├── nasiko/                                # Main package
│   ├── api/v1/ingest.py                   # POST /ingest endpoint
│   ├── app/
│   │   ├── ingestion/                     # R1 — artifact detection
│   │   │   ├── detector.py                #   AST-based framework detector
│   │   │   ├── models.py                  #   IngestionRecord, ArtifactType
│   │   │   └── exceptions.py              #   AmbiguousArtifactError
│   │   ├── utils/
│   │   │   ├── mcp_manifest_generator/    # R3 — manifest generation
│   │   │   │   ├── parser.py              #   AST parser for decorators
│   │   │   │   ├── generator.py           #   Manifest builder
│   │   │   │   └── endpoints.py           #   FastAPI routes
│   │   │   ├── observability/             # R5 — tracing
│   │   │   │   └── mcp_tracing.py         #   OTel + Phoenix integration
│   │   │   ├── mcp_tools.py               # R4 — LangChain/CrewAI wrappers
│   │   │   ├── agent_mcp_linker.py        # R4 — agent linking
│   │   │   └── orchestrate_state.py       # R4 — state management
│   │   ├── agent_builder.py               # Gateway env injection
│   │   └── redis_stream_listener.py       # Event listener
│   ├── mcp_bridge/                        # R2 — STDIO-to-HTTP bridge
│   │   ├── server.py                      #   FastAPI app + BridgeServer
│   │   ├── kong.py                        #   Kong Admin API registrar
│   │   └── models.py                      #   BridgeConfig model
│   ├── docker-compose.local.yml           # Full stack deployment
│   └── litellm-config.yaml                # LLM gateway configuration
├── examples/
│   ├── mcp-calculator-server/             # Sample MCP server (stdio)
│   └── langchain-gateway-agent/           # Sample agent using gateway
├── tests/                                 # 121 tests
│   ├── ingestion/                         # R1 detector tests
│   ├── manifest_generator/                # R3 parser/generator tests
│   ├── bridge/                            # R2 bridge + Kong tests
│   ├── observability/                     # R5 tracing tests
│   ├── orchestration/                     # R4 linker tests
│   └── integration/                       # E2E pipeline tests
├── demo/                                  # Demo scripts
│   ├── demo_local.py                      # In-process demo (no Docker)
│   └── run_demo.sh                        # Live Docker demo
├── docs/                                  # Developer documentation
│   ├── publish-mcp-server.md              # How to publish an MCP server
│   └── llm-gateway.md                     # How to use the LLM gateway
├── Dockerfile                             # Container build
├── pyproject.toml                         # Dependencies
├── conftest.py                            # Test fixtures (Phoenix mocks)
└── README.md                              # This file
```

---

## Quick Start

### Run Tests (no Docker needed)

```bash
pip install -e ".[test]"
pytest tests/ -v
```

### Run Local Demo (no Docker needed)

```bash
pip install fastapi httpx uvicorn pydantic
python demo/demo_local.py
```

### Run Full Stack (Docker)

```bash
# Using Makefile (mirrors make start-nasiko from upstream)
make start-local

# Or directly with docker-compose
docker compose -f nasiko/docker-compose.local.yml up -d

# Run the demo
make demo
```

Services started:
- **Phoenix** at http://localhost:6006 (traces UI)
- **Kong Admin** at http://localhost:8001
- **LLM Gateway** at http://localhost:4000
- **Nasiko Server** at http://localhost:8000

---

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/ingest` | Upload a zip file — auto-detects agent vs MCP server |
| `POST` | `/manifest/generate` | Generate MCP manifest from source path |
| `GET` | `/manifest/{id}` | Retrieve a previously generated manifest |
| `POST` | `/mcp/{id}/start` | Spawn MCP bridge subprocess |
| `GET` | `/mcp/{id}/health` | Check if bridge subprocess is alive |
| `POST` | `/mcp/{id}/call` | Proxy a tool call to the MCP server |
| `POST` | `/agent/link` | Link an agent to an MCP server |

---

## Test Coverage

| Category | Count | What's tested |
|----------|-------|---------------|
| Bridge unit tests | 24 | Port scanning, handshake, subprocess, call_tool proxy |
| Bridge integration | 6 | Real subprocess STDIO protocol, stderr isolation |
| Constraint enforcement | 5 | AST-verified: no shell=True, no eval/exec, flush after write |
| Kong registrar | 3 | HTTP payloads, error handling, fail-fast |
| FastAPI routes | 5 | Route existence, idempotency guard (409), zombie cleanup |
| Observability | 11 | NullSpan, span attributes, result/error recording, kill-switch |
| Ingestion | 15 | Detection (MCP/LangChain/CrewAI), structure validation, ambiguity |
| Manifest generator | 14 | Parser (tools/resources/prompts), generator, schemas, round-trip |
| Integration (E2E) | 9 | Full R1→R3→R4 pipeline, manifest persistence, linker workflow |
| **Required integration** | **15** | **Problem statement cases: valid upload, missing main, ambiguous artifact, manifest contents, gateway acceptance** |
| Orchestration | 3 | Linker status, zero-code injection, traceparent headers |
| **Total** | **121** | |

---

## Documentation

- [How to Publish an MCP Server](docs/publish-mcp-server.md)
- [How to Use the LLM Gateway](docs/llm-gateway.md)

---

## What's NOT Changed

- ✅ Existing agent upload paths (LangChain, CrewAI) — behavior unchanged
- ✅ HTTP API surface (`/api/v1/agents/upload`, `/agents/upload-directory`) — backward-compatible
- ✅ Agent project structure contract — no new requirements
- ✅ Existing AgentCard generator — untouched, byte-identical output
- ✅ Existing trace/metric formats — gateway spans added, existing spans untouched
- ✅ Kong routing for agents — MCP routes are separate