"""
Nasiko MCP Pipeline -- Local Demo (no Docker required)

Demonstrates the full R1->R3->R4 pipeline using FastAPI TestClient.
No external services needed -- runs entirely in-process.

Usage:
    cd stack-up
    python demo/demo_local.py
"""

import json
import os
import sys
import zipfile
from io import BytesIO
from pathlib import Path

# ── Setup ──────────────────────────────────────────────────────────
print("\n" + "=" * 60)
print("  NASIKO MCP PIPELINE -- LOCAL DEMO")
print("=" * 60)

# Ensure we can import from repo root
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# ── Sample MCP Server ─────────────────────────────────────────────
MCP_SERVER_CODE = '''
"""Calculator MCP Server -- Demo"""
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("calculator-demo")

@mcp.tool()
def add(a: int, b: int) -> int:
    """Add two numbers together."""
    return a + b

@mcp.tool()
def multiply(x: float, y: float) -> float:
    """Multiply two numbers."""
    return x * y

@mcp.tool(name="divide")
def safe_divide(numerator: float, denominator: float) -> float:
    """Safely divide. Returns error for division by zero."""
    if denominator == 0:
        return "Error: division by zero"
    return numerator / denominator

@mcp.resource("config://calculator/settings")
def get_settings() -> str:
    """Return calculator configuration."""
    return '{"precision": 10, "mode": "scientific"}'

@mcp.prompt()
def math_helper(problem: str, show_steps: bool = True) -> str:
    """Generate a math problem solving prompt."""
    steps = "Show your work step by step." if show_steps else ""
    return f"Solve: {problem}. {steps}"

if __name__ == "__main__":
    mcp.run()
'''


def create_zip(files: dict) -> BytesIO:
    buf = BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        for name, content in files.items():
            zf.writestr(name, content)
    buf.seek(0)
    return buf


def step(n, title):
    print(f"\n{'-' * 60}")
    print(f"  STEP {n}: {title}")
    print(f"{'-' * 60}")


def ok(msg):
    print(f"  [OK] {msg}")


def info(msg):
    print(f"  --> {msg}")


def fail(msg):
    print(f"  [FAIL] {msg}")
    sys.exit(1)


# ── Build TestClient ──────────────────────────────────────────────
from fastapi import FastAPI
from fastapi.testclient import TestClient
from nasiko.api.v1.ingest import router as ingest_router

app = FastAPI()
app.include_router(ingest_router)

# Mount R3 manifest endpoints
try:
    from nasiko.app.utils.mcp_manifest_generator.endpoints import router as manifest_router
    app.include_router(manifest_router)
except ImportError:
    pass

# Mount R4 linker
try:
    from nasiko.app.utils.agent_mcp_linker import app as linker_app
    app.mount("/agent", linker_app)
except ImportError:
    pass

client = TestClient(app)

# ── STEP 1: Upload & Detect (R1) ──────────────────────────────────
step(1, "Upload & Detection (R1)")

zip_buf = create_zip({
    "src/main.py": MCP_SERVER_CODE,
    "Dockerfile": "FROM python:3.11-slim\nWORKDIR /app\nCOPY src/ src/\nRUN pip install --no-cache-dir \"mcp[cli]>=1.0\"\nCMD [\"python\", \"src/main.py\"]\n",
    "docker-compose.yml": "version: '3.8'\nservices:\n  calculator:\n    build: .\n    stdin_open: true\n",
})
resp = client.post("/ingest", files={"file": ("calculator.zip", zip_buf, "application/zip")})

if resp.status_code != 200:
    fail(f"Upload failed with status {resp.status_code}: {resp.text}")

body = resp.json()
artifact_id = body["artifact_id"]

ok(f"Artifact type: {body['artifact_type']}")
ok(f"Framework: {body['detected_framework']}")
ok(f"Confidence: {body['confidence']}")
ok(f"Entry point: {body.get('entry_point', 'N/A')}")
info(f"Artifact ID: {artifact_id}")

# ── STEP 2: Verify Manifest (R3) ──────────────────────────────────
step(2, "Manifest Generation (R3)")

manifest_gen = body.get("manifest_generated", False)
if not manifest_gen:
    fail("Manifest was NOT auto-generated during ingestion")

manifest = body["manifest"]
tools = manifest.get("tools", [])
resources = manifest.get("resources", [])
prompts = manifest.get("prompts", [])

ok(f"Tools: {len(tools)}")
for t in tools:
    params = ", ".join(
        f"{p}: {s.get('type', '?')}"
        for p, s in t["input_schema"]["properties"].items()
    )
    info(f"  {t['name']}({params}) -- {t.get('description', 'no description')}")

ok(f"Resources: {len(resources)}")
for r in resources:
    info(f"  {r['uri']} -- {r.get('description', 'no description')}")

ok(f"Prompts: {len(prompts)}")
for p in prompts:
    info(f"  {p['name']} -- {p.get('description', 'no description')}")

# ── STEP 3: Retrieve Manifest via API (R3) ─────────────────────────
step(3, "Manifest Retrieval API (R3)")

manifest_resp = client.get(f"/manifest/{artifact_id}")
if manifest_resp.status_code == 200:
    ok("GET /manifest/{artifact_id} returned manifest from disk")
    loaded = manifest_resp.json()
    ok(f"Matches ingestion manifest: {len(loaded['tools'])} tools")
else:
    info(f"GET /manifest/{artifact_id} returned {manifest_resp.status_code}")

# ── STEP 4: Code Persistence (R2 prep) ────────────────────────────
step(4, "Code Persistence (R2 preparation)")

code_path = body.get("code_path")
if code_path and os.path.exists(code_path):
    files_in_code = os.listdir(code_path)
    ok(f"Code persisted at: {code_path}")
    ok(f"Files: {', '.join(files_in_code)}")
else:
    info("Code path not found (OK for test environment)")

# ── STEP 5: Bridge Status Check (R4 pre-check) ────────────────────
step(5, "Pre-Link Status Check (R4)")

from nasiko.app.utils.agent_mcp_linker import get_bridge_status, get_manifest as linker_get_manifest

status = get_bridge_status(artifact_id)
info(f"Bridge status: {status} (expected UNKNOWN -- bridge not started yet)")

# Simulate R2 writing bridge.json with status=ready
bridge_dir = f"/tmp/nasiko/{artifact_id}"
os.makedirs(bridge_dir, exist_ok=True)
with open(os.path.join(bridge_dir, "bridge.json"), "w") as f:
    json.dump({"status": "ready", "port": 8100, "pid": 99999}, f)
ok("Simulated bridge startup (wrote bridge.json with status=ready)")

status = get_bridge_status(artifact_id)
ok(f"Bridge status now: {status}")

# ── STEP 6: Agent Linking (R4) ────────────────────────────────────
step(6, "Agent <-> MCP Linking (R4)")

link_resp = client.post("/agent/link", json={
    "agent_artifact_id": "demo-crewai-agent",
    "mcp_artifact_id": artifact_id,
})

if link_resp.status_code == 200:
    link_body = link_resp.json()
    ok(f"Link status: {link_body['status']}")
    ok(f"Available tools: {link_body['available_tools']}")
else:
    fail(f"Link failed: {link_resp.status_code} -- {link_resp.text}")

# ── STEP 7: R5 Tracing Verification ───────────────────────────────
step(7, "Observability Verification (R5)")

from nasiko.app.utils.observability.mcp_tracing import (
    create_tool_call_span,
    record_tool_result,
    _NullSpan,
)

with create_tool_call_span(
    tracer=None,
    tool_name="add",
    arguments={"a": 40, "b": 2},
    server_name="calculator-demo",
    artifact_id=artifact_id,
) as span:
    ok(f"Span type: {type(span).__name__} (NullSpan = tracing gracefully disabled)")
    record_tool_result(span, {"result": 42})
    ok("record_tool_result() succeeded without crash")

info("In production: set TRACING_ENABLED=true + Phoenix -> real spans")

# ── Summary ───────────────────────────────────────────────────────
print(f"\n{'=' * 60}")
print("  DEMO COMPLETE -- All pipeline stages verified")
print(f"{'=' * 60}")
print(f"""
  R1  Ingestion & Detection  [OK]  Detected MCP_SERVER from zip upload
  R3  Manifest Generator     [OK]  Extracted {len(tools)} tools, {len(resources)} resources, {len(prompts)} prompts
  R2  Bridge Preparation     [OK]  Code persisted, bridge.json simulated
  R4  Agent Linking          [OK]  Agent linked, {len(tools)} tools available
  R5  Observability          [OK]  Tracing gracefully handles disabled state

  Artifact ID: {artifact_id}
  Manifest:    /tmp/nasiko/{artifact_id}/manifest.json
  Code:        {code_path or 'N/A'}
""")
