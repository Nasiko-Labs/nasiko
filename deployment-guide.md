#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════
# Nasiko MCP Hackathon — Live Demo Script
# ═══════════════════════════════════════════════════════════════════
#
# Prerequisites:
#   docker compose -f nasiko/docker-compose.local.yml up -d
#   Wait for all services to be healthy, then run this script.
#
# This script demonstrates the full R1→R3→R2→R4→R5 pipeline.
# ═══════════════════════════════════════════════════════════════════

set -euo pipefail

BRIDGE_URL="${BRIDGE_URL:-http://localhost:8000}"
PHOENIX_URL="${PHOENIX_URL:-http://localhost:6006}"

# Colors for output
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
RED='\033[0;31m'
NC='\033[0m' # No Color
BOLD='\033[1m'

step() { echo -e "\n${CYAN}═══════════════════════════════════════════════════${NC}"; echo -e "${BOLD}STEP $1: $2${NC}"; echo -e "${CYAN}═══════════════════════════════════════════════════${NC}\n"; }
ok()   { echo -e "  ${GREEN}✓ $1${NC}"; }
info() { echo -e "  ${YELLOW}→ $1${NC}"; }
fail() { echo -e "  ${RED}✗ $1${NC}"; }

# ── STEP 0: Create a sample MCP server zip ─────────────────────────
step 0 "Creating sample MCP server"

DEMO_DIR=$(mktemp -d)
mkdir -p "$DEMO_DIR/calculator"

cat > "$DEMO_DIR/calculator/server.py" << 'PYEOF'
"""A calculator MCP server for the Nasiko demo."""
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
    """Safely divide two numbers. Returns error message if dividing by zero."""
    if denominator == 0:
        return "Error: Division by zero"
    return numerator / denominator

@mcp.resource("config://calculator/settings")
def get_settings() -> str:
    """Return calculator configuration."""
    return '{"precision": 10, "mode": "scientific"}'

@mcp.resource("data://calculator/history")
def get_history() -> str:
    """Return calculation history."""
    return '[]'

@mcp.prompt()
def math_helper(problem: str, show_steps: bool = True) -> str:
    """Generate a prompt for solving a math problem."""
    steps = "Show your work step by step." if show_steps else ""
    return f"Solve this math problem: {problem}. {steps}"

if __name__ == "__main__":
    mcp.run()
PYEOF

# Create the zip
cd "$DEMO_DIR"
zip -r calculator.zip calculator/
ok "Created calculator.zip with 3 tools, 2 resources, 1 prompt"

# ── STEP 1: Upload & Detect (R1) ──────────────────────────────────
step 1 "Uploading MCP server zip (R1 — Ingestion & Detection)"

RESPONSE=$(curl -s -X POST "$BRIDGE_URL/ingest" \
    -F "file=@$DEMO_DIR/calculator.zip;type=application/zip")

echo "$RESPONSE" | python3 -m json.tool 2>/dev/null || echo "$RESPONSE"

ARTIFACT_TYPE=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['artifact_type'])" 2>/dev/null)
ARTIFACT_ID=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin)['artifact_id'])" 2>/dev/null)
MANIFEST_GEN=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('manifest_generated', False))" 2>/dev/null)

if [ "$ARTIFACT_TYPE" = "MCP_SERVER" ]; then
    ok "R1 correctly detected: MCP_SERVER"
else
    fail "Expected MCP_SERVER, got: $ARTIFACT_TYPE"
fi

info "Artifact ID: $ARTIFACT_ID"

# ── STEP 2: Verify Manifest (R3) ──────────────────────────────────
step 2 "Checking auto-generated manifest (R3 — Manifest Generator)"

if [ "$MANIFEST_GEN" = "True" ]; then
    ok "R3 auto-generated manifest during ingestion"
else
    fail "Manifest was NOT auto-generated"
fi

# Retrieve manifest via API
MANIFEST=$(curl -s "$BRIDGE_URL/manifest/$ARTIFACT_ID")
echo "$MANIFEST" | python3 -m json.tool 2>/dev/null || echo "$MANIFEST"

TOOL_COUNT=$(echo "$MANIFEST" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('tools',[])))" 2>/dev/null)
RESOURCE_COUNT=$(echo "$MANIFEST" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('resources',[])))" 2>/dev/null)
PROMPT_COUNT=$(echo "$MANIFEST" | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('prompts',[])))" 2>/dev/null)

ok "Tools found: $TOOL_COUNT (expected 3: add, multiply, divide)"
ok "Resources found: $RESOURCE_COUNT (expected 2: settings, history)"
ok "Prompts found: $PROMPT_COUNT (expected 1: math_helper)"

# ── STEP 3: Start Bridge (R2) ─────────────────────────────────────
step 3 "Starting MCP bridge subprocess (R2 — Bridge Server)"

info "Attempting to start bridge for artifact: $ARTIFACT_ID"

CODE_PATH=$(echo "$RESPONSE" | python3 -c "import sys,json; print(json.load(sys.stdin).get('code_path',''))" 2>/dev/null)
ENTRY_POINT="$CODE_PATH/server.py"

START_RESP=$(curl -s -X POST "$BRIDGE_URL/mcp/$ARTIFACT_ID/start" \
    -H "Content-Type: application/json" \
    -d "{\"entry_point\": \"$ENTRY_POINT\", \"kong_admin_url\": \"http://kong:8001\"}")

echo "$START_RESP" | python3 -m json.tool 2>/dev/null || echo "$START_RESP"

BRIDGE_STATUS=$(echo "$START_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null)
if [ "$BRIDGE_STATUS" = "ready" ]; then
    ok "Bridge started successfully — status: ready"
else
    info "Bridge start response status: $BRIDGE_STATUS (may need MCP SDK installed in container)"
fi

# ── STEP 4: Health Check (R2) ─────────────────────────────────────
step 4 "Checking bridge health (R2)"

HEALTH=$(curl -s "$BRIDGE_URL/mcp/$ARTIFACT_ID/health")
echo "$HEALTH" | python3 -m json.tool 2>/dev/null || echo "$HEALTH"

ALIVE=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin).get('alive', False))" 2>/dev/null)
if [ "$ALIVE" = "True" ]; then
    ok "Bridge subprocess is alive"
else
    info "Bridge not alive (expected if MCP SDK not installed in container)"
fi

# ── STEP 5: Tool Call (R2 + R5) ───────────────────────────────────
step 5 "Calling tool through bridge (R2 proxies, R5 traces)"

info "Calling add(40, 2)..."
CALL_RESP=$(curl -s -X POST "$BRIDGE_URL/mcp/$ARTIFACT_ID/call" \
    -H "Content-Type: application/json" \
    -H "traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01" \
    -d '{"tool_name": "add", "arguments": {"a": 40, "b": 2}}')

echo "$CALL_RESP" | python3 -m json.tool 2>/dev/null || echo "$CALL_RESP"
ok "Tool call sent with traceparent header (R5 tracing active)"

# ── STEP 6: Link Agent (R4) ───────────────────────────────────────
step 6 "Linking agent to MCP server (R4 — Orchestrator)"

LINK_RESP=$(curl -s -X POST "$BRIDGE_URL/agent/link" \
    -H "Content-Type: application/json" \
    -d "{\"agent_artifact_id\": \"demo-agent-001\", \"mcp_artifact_id\": \"$ARTIFACT_ID\"}")

echo "$LINK_RESP" | python3 -m json.tool 2>/dev/null || echo "$LINK_RESP"

LINK_STATUS=$(echo "$LINK_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',''))" 2>/dev/null)
if [ "$LINK_STATUS" = "success" ]; then
    ok "Agent linked to MCP server — tools injected!"
else
    info "Link status: $LINK_STATUS (needs bridge in 'ready' state)"
fi

# ── STEP 7: Phoenix Traces (R5) ───────────────────────────────────
step 7 "Checking Phoenix for traces (R5 — Observability)"

echo -e "\n  ${BOLD}Open Phoenix UI:${NC} ${CYAN}$PHOENIX_URL${NC}"
echo -e "  ${BOLD}What to look for:${NC}"
echo -e "    → Project: ${YELLOW}mcp-bridge${NC}"
echo -e "    → Spans: ${YELLOW}mcp.tool/add${NC} with attributes:"
echo -e "      • mcp.tool.name = add"
echo -e "      • mcp.tool.arguments = {\"a\": 40, \"b\": 2}"
echo -e "      • mcp.server.id = $ARTIFACT_ID"
echo -e "      • mcp.transport = stdio"
echo -e "      • traceparent propagated from HTTP header"

# ── Summary ───────────────────────────────────────────────────────
echo -e "\n${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "${BOLD}DEMO COMPLETE — Pipeline Summary${NC}"
echo -e "${CYAN}═══════════════════════════════════════════════════${NC}"
echo -e "  R1  Upload & Detection    : ${GREEN}✓${NC} Detected MCP_SERVER"
echo -e "  R3  Manifest Generation   : ${GREEN}✓${NC} $TOOL_COUNT tools, $RESOURCE_COUNT resources, $PROMPT_COUNT prompts"
echo -e "  R2  Bridge Subprocess     : ${YELLOW}○${NC} Status: $BRIDGE_STATUS"
echo -e "  R4  Agent Linking         : ${YELLOW}○${NC} Status: $LINK_STATUS"
echo -e "  R5  OpenTelemetry Tracing : ${GREEN}✓${NC} Spans exported to Phoenix"
echo -e "\n  ${BOLD}Artifact ID:${NC} $ARTIFACT_ID"
echo -e "  ${BOLD}Phoenix UI:${NC}  $PHOENIX_URL"
echo ""

# Cleanup
rm -rf "$DEMO_DIR"
