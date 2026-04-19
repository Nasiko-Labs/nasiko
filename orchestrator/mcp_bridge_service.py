"""
MCP Bridge Service
Bridges MCP servers (stdio) to HTTP endpoints using FastAPI.
Runs MCP server as subprocess and communicates via JSON over stdin/stdout.
"""

import asyncio
import importlib
import json
import logging
import subprocess
import sys
from pathlib import Path
from typing import Dict, Any, Optional
from pydantic import BaseModel


class ToolCallRequest(BaseModel):
    """Request model for calling an MCP tool"""
    tool_name: str
    arguments: Dict[str, Any] = {}


class ToolCallResponse(BaseModel):
    """Response model for tool calls"""
    success: bool
    result: Any = None
    error: Optional[str] = None


class MCPBridgeService:
    """
    Bridges MCP server (stdio-based) to HTTP via FastAPI.
    
    MCP Protocol (JSON over stdio):
    - Send: {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "...", "arguments": {...}}}
    - Recv: {"jsonrpc": "2.0", "id": 1, "result": {...}}
    """
    
    def __init__(self, mcp_server_path: str, docker_compose_path: Optional[str] = None, logger: Optional[logging.Logger] = None):
        """
        Initialize MCP bridge.
        
        Args:
            mcp_server_path: Path to MCP server directory
            docker_compose_path: Optional path to docker-compose.yml (if different from mcp_server_path)
            logger: Optional logger instance
        """
        # Normalize to absolute paths so subprocess cwd and script path do not duplicate relative segments.
        self.mcp_server_path = Path(mcp_server_path).resolve()
        self.docker_compose_path = (
            Path(docker_compose_path).resolve()
            if docker_compose_path
            else self.mcp_server_path / "docker-compose.yml"
        )
        self.logger = logger or logging.getLogger(__name__)
        
        self.process: Optional[subprocess.Popen] = None
        self.call_counter = 0
        self.running = False
        self.server_id = self.mcp_server_path.name
        self.initialized = False
        self.legacy_mode = False
        self._trace_api = self._load_trace_api()
        self.tracer = (
            self._trace_api.get_tracer("nasiko.mcp.bridge")
            if self._trace_api is not None
            else None
        )

    @staticmethod
    def _load_trace_api():
        """Dynamically load OpenTelemetry trace API if available."""
        try:
            return importlib.import_module("opentelemetry.trace")
        except Exception:
            return None
    
    async def start(self) -> bool:
        """
        Start the MCP server as subprocess using docker-compose.
        
        Returns:
            True if started successfully, False otherwise
        """
        try:
            self.logger.info(f"Starting MCP server from: {self.mcp_server_path}")
            
            # Validate docker-compose exists
            if not self.docker_compose_path.exists():
                self.logger.error(f"docker-compose.yml not found at {self.docker_compose_path}")
                return False
            
            # Start subprocess: run MCP main.py directly or via docker-compose
            # For simplicity in 36-hour hackathon, run Python directly
            main_py = self._find_main_py()
            if not main_py:
                self.logger.error("main.py not found")
                return False
            
            self.logger.info(f"Starting MCP server process with: {main_py}")
            
            self.process = subprocess.Popen(
                [sys.executable, str(main_py)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=False,  # Use binary mode for MCP protocol
                cwd=str(self.mcp_server_path),
                bufsize=0,  # Unbuffered
            )

            # Fail fast if process crashes on startup.
            await asyncio.sleep(0.1)
            if self.process.poll() is not None:
                stderr_preview = ""
                if self.process.stderr:
                    stderr_preview = self.process.stderr.read().decode(
                        "utf-8", errors="ignore"
                    )
                self.logger.error(
                    "MCP server process exited during startup. stderr: %s",
                    stderr_preview.strip(),
                )
                self.running = False
                return False
            
            self.running = True
            self.logger.info("MCP server started successfully")
            return True
        
        except Exception as e:
            self.logger.error(f"Failed to start MCP server: {e}")
            self.running = False
            return False
    
    async def stop(self) -> bool:
        """
        Stop the MCP server process.
        
        Returns:
            True if stopped successfully, False otherwise
        """
        try:
            if self.process:
                self.logger.info("Stopping MCP server process")
                self.process.terminate()
                
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.logger.warning("MCP process did not terminate, killing...")
                    self.process.kill()
                    self.process.wait()

                if self.process.stdin:
                    self.process.stdin.close()
                if self.process.stdout:
                    self.process.stdout.close()
                if self.process.stderr:
                    self.process.stderr.close()
                
                self.running = False
                self.initialized = False
                self.legacy_mode = False
                self.process = None
                self.logger.info("MCP server stopped")
                return True
            
            return False
        
        except Exception as e:
            self.logger.error(f"Error stopping MCP server: {e}")
            return False
    
    def _find_main_py(self) -> Optional[Path]:
        """Find main.py entry point"""
        locations = [
            self.mcp_server_path / "src" / "main.py",
            self.mcp_server_path / "main.py",
            self.mcp_server_path / "src" / "__main__.py",
            self.mcp_server_path / "__main__.py",
        ]
        
        for loc in locations:
            if loc.exists():
                return loc
        
        return None
    
    def _build_jsonrpc_request(self, method: str, params: Dict[str, Any]) -> str:
        """Build JSON-RPC request"""
        self.call_counter += 1
        request = {
            "jsonrpc": "2.0",
            "id": self.call_counter,
            "method": method,
            "params": params,
        }
        return json.dumps(request) + "\n"

    async def _send_jsonrpc_request(
        self,
        method: str,
        params: Dict[str, Any],
        *,
        timeout: float = 30.0,
        expect_response: bool = True,
    ) -> Optional[Dict[str, Any]]:
        """Send one JSON-RPC request to the MCP subprocess and parse one response."""
        if not self.process or not self.process.stdin:
            raise RuntimeError("MCP process stdin is not available")

        request = self._build_jsonrpc_request(method, params)
        self.logger.debug("Sending MCP request: %s", request.strip())

        self.process.stdin.write(request.encode("utf-8"))
        self.process.stdin.flush()

        if not expect_response:
            return None

        response_payload = await asyncio.wait_for(
            self._read_jsonrpc_response(), timeout=timeout
        )
        self.logger.debug("Received MCP response: %s", response_payload)
        return response_payload

    async def _send_jsonrpc_notification(self, method: str, params: Dict[str, Any]) -> None:
        """Send a JSON-RPC notification (no response expected)."""
        if not self.process or not self.process.stdin:
            raise RuntimeError("MCP process stdin is not available")

        notification = {
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }
        payload = json.dumps(notification) + "\n"
        self.logger.debug("Sending MCP notification: %s", payload.strip())
        self.process.stdin.write(payload.encode("utf-8"))
        self.process.stdin.flush()

    async def _ensure_initialized(self) -> bool:
        """
        Initialize MCP session for official SDK servers.

        Falls back to legacy mode for simple demo servers that only implement tools/call.
        """
        if self.initialized or self.legacy_mode:
            return True

        try:
            init_response = await self._send_jsonrpc_request(
                "initialize",
                {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "nasiko-mcp-bridge", "version": "1.0.0"},
                },
                timeout=10.0,
            )

            if init_response is None:
                self.legacy_mode = True
                return True

            error = init_response.get("error")
            if error:
                code = error.get("code")
                # Legacy servers can reject initialize with method-not-found.
                if code == -32601:
                    self.logger.info(
                        "MCP server does not support initialize; switching bridge to legacy mode"
                    )
                    self.legacy_mode = True
                    return True

                self.logger.error("MCP initialize failed: %s", error.get("message"))
                return False

            await self._send_jsonrpc_notification("notifications/initialized", {})
            self.initialized = True
            self.logger.info("MCP session initialized for server '%s'", self.server_id)
            return True

        except asyncio.TimeoutError:
            self.logger.warning(
                "MCP initialize timed out; falling back to legacy mode for server '%s'",
                self.server_id,
            )
            self.legacy_mode = True
            return True
        except Exception as e:
            self.logger.warning(
                "MCP initialize failed (%s); falling back to legacy mode for server '%s'",
                e,
                self.server_id,
            )
            self.legacy_mode = True
            return True
    
    async def call_tool(self, tool_name: str, arguments: Dict[str, Any]) -> ToolCallResponse:
        """
        Call an MCP tool via subprocess stdio.
        
        Args:
            tool_name: Name of the tool to call
            arguments: Tool arguments
            
        Returns:
            ToolCallResponse with result or error
        """
        if not self.running or not self.process:
            self.logger.error("MCP server not running")
            return ToolCallResponse(
                success=False,
                error="MCP server not running",
            )

        if self.process.poll() is not None:
            self.running = False
            return ToolCallResponse(
                success=False,
                error="MCP server process has exited",
            )
        
        try:
            span_cm = (
                self.tracer.start_as_current_span("mcp.tool.call")
                if self.tracer is not None
                else None
            )

            if span_cm is not None:
                span_cm.__enter__()
                current_span = self._trace_api.get_current_span()
                current_span.set_attribute("mcp.server.id", self.server_id)
                current_span.set_attribute("mcp.tool.name", tool_name)
                current_span.set_attribute("mcp.call.id", self.call_counter + 1)
                current_span.set_attribute("mcp.transport", "stdio")
                current_span.set_attribute("mcp.arguments.size", len(json.dumps(arguments)))

            self.logger.info(f"Calling MCP tool: {tool_name} with args: {arguments}")

            if not await self._ensure_initialized():
                return ToolCallResponse(success=False, error="Failed to initialize MCP session")

            try:
                response = await self._send_jsonrpc_request(
                    "tools/call",
                    {"name": tool_name, "arguments": arguments},
                    timeout=30.0,
                )

                # Check for errors in response
                if "error" in response:
                    error_msg = response["error"].get("message", "Unknown error")
                    self.logger.error(f"MCP error: {error_msg}")
                    if self.tracer is not None:
                        self._trace_api.get_current_span().set_attribute("mcp.call.success", False)
                        self._trace_api.get_current_span().set_attribute("mcp.call.error", error_msg)
                    return ToolCallResponse(success=False, error=error_msg)

                if self.tracer is not None:
                    self._trace_api.get_current_span().set_attribute("mcp.call.success", True)

                result_payload = response.get("result")

                # Normalize official FastMCP tool call shape.
                if isinstance(result_payload, dict) and "structuredContent" in result_payload:
                    structured = result_payload.get("structuredContent")
                    content = result_payload.get("content")
                    is_error = bool(result_payload.get("isError", False))

                    if is_error:
                        error_text = "Tool reported error"
                        if isinstance(content, list) and content:
                            first = content[0]
                            if isinstance(first, dict) and first.get("text"):
                                error_text = str(first["text"])
                        return ToolCallResponse(success=False, error=error_text, result=result_payload)

                    if structured is not None:
                        return ToolCallResponse(success=True, result=structured)

                return ToolCallResponse(success=True, result=result_payload)

            except asyncio.TimeoutError:
                self.logger.error("MCP tool call timed out")
                if self.tracer is not None:
                    self._trace_api.get_current_span().set_attribute("mcp.call.success", False)
                    self._trace_api.get_current_span().set_attribute("mcp.call.error", "timeout")
                return ToolCallResponse(
                    success=False,
                    error="Tool call timed out (30 seconds)",
                )

            finally:
                if span_cm is not None:
                    span_cm.__exit__(None, None, None)
        
        except Exception as e:
            self.logger.error(f"Error calling MCP tool: {e}")
            if self.tracer is not None:
                try:
                    self._trace_api.get_current_span().set_attribute("mcp.call.success", False)
                    self._trace_api.get_current_span().set_attribute("mcp.call.error", str(e))
                except Exception:
                    pass
            return ToolCallResponse(
                success=False,
                error=str(e),
            )
    
    async def _read_jsonrpc_response(self) -> Dict[str, Any]:
        """
        Read and parse one JSON-RPC response from process stdout.

        Some MCP servers or wrappers may write non-protocol log lines to stdout.
        This reader skips blank/non-JSON/non-object lines until a JSON object is found.
        """
        loop = asyncio.get_running_loop()

        def read_line() -> bytes:
            if not self.process or not self.process.stdout:
                return b""
            return self.process.stdout.readline()

        while True:
            raw_line = await loop.run_in_executor(None, read_line)
            if not raw_line:
                raise RuntimeError("MCP server closed stdout before sending a response")

            line = raw_line.decode("utf-8", errors="ignore").strip()
            if not line:
                continue

            try:
                payload = json.loads(line)
            except json.JSONDecodeError:
                self.logger.debug(
                    "Skipping non-JSON stdout line from MCP server: %s", line
                )
                continue

            if not isinstance(payload, dict):
                self.logger.debug(
                    "Skipping non-object JSON stdout payload from MCP server: %s",
                    payload,
                )
                continue

            return payload
    
    async def health_check(self) -> bool:
        """Check if MCP server is healthy"""
        return self.running and self.process and self.process.poll() is None
