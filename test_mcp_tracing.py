"""BridgeServer — STDIO-to-HTTP bridge for MCP agent subprocesses.

This is the critical-path component (R2).  It spawns a Python MCP agent as
a subprocess, performs the JSON-RPC 2.0 initialize handshake over STDIO,
registers the resulting service with Kong, and exposes FastAPI endpoints for
health checks and proxied tool calls.
"""

from __future__ import annotations

import json
import socket
import subprocess
import time
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel

from nasiko.mcp_bridge.kong import KongRegistrar
from nasiko.mcp_bridge.models import BridgeConfig

try:
    # Prefer the local module: uses standard OTel SDK → Phoenix OTLP HTTP (port 4318).
    # No arize-phoenix pip dependency needed, no numpy build failures.
    from nasiko.app.utils.observability.mcp_tracing_local import (
        bootstrap_mcp_tracing,
        instrument_mcp_bridge,
        create_tool_call_span,
        record_tool_result,
        record_tool_error,
    )
except ImportError:
    # Fallback: original module (uses phoenix.otel.register)
    from nasiko.app.utils.observability.mcp_tracing import (
        bootstrap_mcp_tracing,
        instrument_mcp_bridge,
        create_tool_call_span,
        record_tool_result,
        record_tool_error,
    )


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# Custom exceptions
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━


class BridgeStartError(Exception):
    """Raised when the bridge subprocess fails to start or initialize."""


class MCPHandshakeError(Exception):
    """Raised when the MCP JSON-RPC initialize handshake fails."""


class MCPToolCallError(Exception):
    """Raised when a proxied tools/call returns a JSON-RPC error."""


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# BridgeServer
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━


class BridgeServer:
    """Manages a single MCP agent subprocess and its Kong registration."""

    def __init__(
        self,
        artifact_id: str,
        entry_point: str,
        kong_admin_url: str = "http://localhost:8001",
    ) -> None:
        self.artifact_id = artifact_id
        self.entry_point = entry_point
        self.kong_admin_url = kong_admin_url
        self._proc: subprocess.Popen[bytes] | None = None
        self._call_id: int = 1  # auto-incrementing JSON-RPC request id

    # ------------------------------------------------------------------
    # Port discovery
    # ------------------------------------------------------------------

    @staticmethod
    def _find_free_port() -> int:
        """Scan ports 8100–8200 inclusive and return the first available one.

        Raises:
            RuntimeError: If every port in the range is already bound.
        """
        for port in range(8100, 8201):
            try:
                s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                s.bind(("", port))
                s.close()
                return port
            except OSError:
                continue
        raise RuntimeError("No free port in range 8100-8200")

    # ------------------------------------------------------------------
    # MCP JSON-RPC 2.0 handshake
    # ------------------------------------------------------------------

    @staticmethod
    def _perform_mcp_handshake(proc: subprocess.Popen[bytes]) -> None:
        """Execute the three-step MCP initialize handshake over STDIO.

        Sequence:
            1. Send ``initialize`` request   → stdin
            2. Read ``initialize`` response   ← stdout
            3. Send ``notifications/initialized`` notification → stdin

        Raises:
            MCPHandshakeError: On any protocol violation.
        """
        assert proc.stdin is not None
        assert proc.stdout is not None

        # ── Step 1: initialize request ──────────────────────────────────
        init_request: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {
                    "roots": {"listChanged": True},
                    "sampling": {},
                },
                "clientInfo": {
                    "name": "NasikoBridge",
                    "version": "1.0.0",
                },
            },
        }
        proc.stdin.write((json.dumps(init_request) + "\n").encode())
        proc.stdin.flush()

        # ── Step 2: read initialize response ────────────────────────────
        raw_line = proc.stdout.readline()
        if not raw_line:
            raise MCPHandshakeError(
                "Agent closed stdout before sending initialize response"
            )

        try:
            response = json.loads(raw_line)
        except json.JSONDecodeError as exc:
            raise MCPHandshakeError(
                f"Invalid JSON in initialize response: {raw_line!r}"
            ) from exc

        if response.get("jsonrpc") != "2.0":
            raise MCPHandshakeError(
                f"Bad jsonrpc version in response: {raw_line.decode()}"
            )
        if response.get("id") != 1:
            raise MCPHandshakeError(
                f"Unexpected id in initialize response: {raw_line.decode()}"
            )
        if "result" not in response:
            raise MCPHandshakeError(
                f"Initialize response contains error or missing result: "
                f"{raw_line.decode()}"
            )

        # ── Step 3: initialized notification (no "id" field!) ───────────
        notification: dict[str, str] = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }
        proc.stdin.write((json.dumps(notification) + "\n").encode())
        proc.stdin.flush()

    # ------------------------------------------------------------------
    # Lifecycle
    # ------------------------------------------------------------------

    def start(self) -> BridgeConfig:
        """Spawn agent subprocess, handshake, register with Kong, persist config.

        Returns:
            A fully populated ``BridgeConfig``.

        Raises:
            BridgeStartError: If the subprocess dies or the handshake fails.
        """
        # 1. Find a free port
        port = self._find_free_port()

        # 2. Spawn subprocess — NO shell=True, unbuffered I/O
        proc = subprocess.Popen(
            ["python", self.entry_point],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            bufsize=0,
        )
        self._proc = proc

        # 3. Let the process stabilize
        time.sleep(1)
        if proc.poll() is not None:
            stderr_output = proc.stderr.read().decode() if proc.stderr else ""
            raise BridgeStartError(
                f"Agent process exited immediately, stderr: {stderr_output}"
            )

        # 4. MCP handshake
        try:
            self._perform_mcp_handshake(proc)
        except MCPHandshakeError as exc:
            raise BridgeStartError(str(exc)) from exc

        # 5. Register with Kong
        registrar = KongRegistrar(self.kong_admin_url)
        kong_service_id, kong_route_id = registrar.register(
            self.artifact_id, port
        )

        # 6. Build config
        config = BridgeConfig(
            artifact_id=self.artifact_id,
            port=port,
            entry_point=self.entry_point,
            pid=proc.pid,
            kong_service_id=kong_service_id,
            kong_route_id=kong_route_id,
            status="ready",
            created_at=datetime.now(UTC),
            bridge_json_path=f"/tmp/nasiko/{self.artifact_id}/bridge.json",
        )

        # 7. Persist to disk
        path = Path(f"/tmp/nasiko/{self.artifact_id}")
        path.mkdir(parents=True, exist_ok=True)
        (path / "bridge.json").write_text(config.model_dump_json())

        # 8. Return
        return config

    # ------------------------------------------------------------------
    # Tool call proxy
    # ------------------------------------------------------------------

    def call_tool(self, tool_name: str, arguments: dict[str, Any]) -> dict[str, Any]:
        """Proxy a JSON-RPC ``tools/call`` to the running agent.

        Returns:
            The full parsed JSON-RPC response dict.

        Raises:
            MCPToolCallError: If the response contains an ``"error"`` key.
        """
        if self._proc is None or self._proc.stdin is None or self._proc.stdout is None:
            raise MCPToolCallError("Bridge subprocess is not running")

        self._call_id += 1
        request: dict[str, Any] = {
            "jsonrpc": "2.0",
            "id": self._call_id,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments,
            },
        }
        self._proc.stdin.write((json.dumps(request) + "\n").encode())
        self._proc.stdin.flush()

        raw_line = self._proc.stdout.readline()
        if not raw_line:
            raise MCPToolCallError("Agent closed stdout before responding")

        response: dict[str, Any] = json.loads(raw_line)
        if "error" in response:
            raise MCPToolCallError(str(response["error"]))

        return response


# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
# FastAPI application
# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

app = FastAPI(title="Nasiko MCP Bridge")

# Mount R1 ingest endpoint so /ingest is reachable
try:
    from nasiko.api.v1.ingest import router as ingest_router
    app.include_router(ingest_router)
except ImportError:
    pass

instrument_mcp_bridge(app)
_tracer = bootstrap_mcp_tracing("mcp-bridge")

_bridges: dict[str, BridgeServer] = {}


class StartRequest(BaseModel):
    entry_point: str
    kong_admin_url: str = "http://localhost:8001"


class ToolCallRequest(BaseModel):
    tool_name: str
    arguments: dict[str, Any]


@app.post("/mcp/{artifact_id}/start")
def start_bridge(artifact_id: str, body: StartRequest) -> dict[str, Any]:
    """Spawn an MCP agent subprocess, handshake, and register with Kong.

    Idempotency: if a bridge already exists for this artifact_id and the
    subprocess is still alive, return 409 instead of spawning a duplicate.
    """
    # ── Guard: prevent duplicate bridges / zombie leaks ──────────────
    if artifact_id in _bridges:
        existing = _bridges[artifact_id]
        if existing._proc is not None and existing._proc.poll() is None:
            raise HTTPException(
                status_code=409,
                detail=f"Bridge for '{artifact_id}' is already running",
            )
        # Process is dead — clean up stale entry and allow re-start
        del _bridges[artifact_id]

    try:
        bridge = BridgeServer(artifact_id, body.entry_point, body.kong_admin_url)
        config = bridge.start()
        _bridges[artifact_id] = bridge
        return config.model_dump()
    except Exception as exc:
        raise HTTPException(status_code=500, detail=str(exc)) from exc


@app.get("/mcp/{artifact_id}/health")
def health_check(artifact_id: str) -> dict[str, Any]:
    """Check whether the agent subprocess is still alive."""
    if artifact_id not in _bridges:
        raise HTTPException(status_code=404, detail="Bridge not found")
    proc = _bridges[artifact_id]._proc
    alive = proc is not None and proc.poll() is None
    return {"artifact_id": artifact_id, "alive": alive}


@app.post("/mcp/{artifact_id}/call")
def call_tool(artifact_id: str, body: ToolCallRequest) -> dict[str, Any]:
    """Proxy a tool call to the running MCP agent."""
    if artifact_id not in _bridges:
        raise HTTPException(status_code=404, detail="Bridge not found")
    with create_tool_call_span(
        tracer=_tracer,
        tool_name=body.tool_name,
        arguments=body.arguments,
        server_name=artifact_id,
        artifact_id=artifact_id,
    ) as span:
        try:
            result = _bridges[artifact_id].call_tool(body.tool_name, body.arguments)
            record_tool_result(span, result)
            return result
        except MCPToolCallError as exc:
            record_tool_error(span, exc)
            raise HTTPException(status_code=500, detail=str(exc)) from exc
