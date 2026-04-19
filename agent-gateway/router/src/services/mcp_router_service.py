"""
MCP Router Service
Handles HTTP routing to MCP servers via bridge instances.
"""

import logging
import json
import sys
from typing import Dict, Any, Optional
from pathlib import Path
from pydantic import BaseModel

# Make repository root importable when router runs as an isolated service.
REPO_ROOT = Path(__file__).resolve().parents[4]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

try:
    from orchestrator.mcp_bridge_service import MCPBridgeService
except Exception:
    MCPBridgeService = None


class ToolCallRequest(BaseModel):
    """Request model for MCP tool calls"""
    tool_name: str
    arguments: Dict[str, Any] = {}


class ToolCallResponse(BaseModel):
    """Response model for tool calls"""
    success: bool
    result: Any = None
    error: Optional[str] = None


class MCPRouterService:
    """Routes MCP tool calls to bridge instances"""
    
    def __init__(self, logger: Optional[logging.Logger] = None):
        self.logger = logger or logging.getLogger(__name__)
        self.bridges: Dict[str, Any] = {}  # Maps server_name -> MCPBridgeService
        self.agents_directory = self._resolve_agents_directory()

    def _resolve_agents_directory(self) -> Path:
        """Resolve agents directory across local/dev/container layouts."""
        candidates = [
            Path("agents"),
            REPO_ROOT / "agents",
            Path("/app/agents"),
        ]

        for candidate in candidates:
            if candidate.exists() and candidate.is_dir():
                self.logger.info(f"Using agents directory: {candidate}")
                return candidate

        # Fall back to local relative path; callers get clear not-found errors per server.
        fallback = Path("agents")
        self.logger.warning(
            f"Could not auto-detect agents directory, falling back to: {fallback}"
        )
        return fallback

    def _resolve_manifest_path(self, server_id: str) -> Optional[Path]:
        """
        Resolve MCP manifest path for a server.

        Supports both:
        - agents/<server_id>/McpServerManifest.json
        - agents/<server_id>/<version>/McpServerManifest.json
        """
        server_dir = self.agents_directory / server_id
        if not server_dir.exists() or not server_dir.is_dir():
            return None

        root_manifest = server_dir / "McpServerManifest.json"
        if root_manifest.exists() and root_manifest.is_file():
            return root_manifest

        version_manifests = [
            path
            for path in server_dir.glob("*/McpServerManifest.json")
            if path.is_file()
        ]
        if not version_manifests:
            return None

        return max(version_manifests, key=lambda path: path.parent.stat().st_mtime)

    def _resolve_server_runtime_path(self, server_id: str) -> Path:
        """
        Resolve runtime folder used to start the MCP process.

        If versioned folders exist, pick the most recently updated version folder.
        """
        server_dir = self.agents_directory / server_id
        if not server_dir.exists() or not server_dir.is_dir():
            return server_dir

        root_main = server_dir / "src" / "main.py"
        if root_main.exists():
            return server_dir

        candidates = [
            path
            for path in server_dir.iterdir()
            if path.is_dir() and (path / "src" / "main.py").exists()
        ]
        if not candidates:
            return server_dir

        return max(candidates, key=lambda path: path.stat().st_mtime)

    async def _get_or_start_bridge(self, server_id: str):
        """Get existing bridge or start a new one for an MCP server."""
        existing = self.bridges.get(server_id)
        if existing:
            try:
                if await existing.health_check():
                    return existing
            except Exception as e:
                self.logger.warning(
                    f"Existing bridge health check failed for {server_id}: {e}"
                )

        if MCPBridgeService is None:
            raise RuntimeError("MCP bridge service is unavailable in router runtime")

        server_path = self._resolve_server_runtime_path(server_id)
        if not server_path.exists() or not server_path.is_dir():
            raise FileNotFoundError(f"MCP server folder not found: {server_path}")

        bridge = MCPBridgeService(str(server_path), logger=self.logger)
        started = await bridge.start()
        if not started:
            raise RuntimeError(f"Failed to start MCP bridge for server '{server_id}'")

        self.bridges[server_id] = bridge
        return bridge

    async def stop_bridge(self, server_id: str) -> bool:
        """Stop a single MCP bridge if running."""
        bridge = self.bridges.get(server_id)
        if not bridge:
            return False

        try:
            stopped = await bridge.stop()
            self.bridges.pop(server_id, None)
            return stopped
        except Exception as e:
            self.logger.error(f"Failed to stop bridge for {server_id}: {e}")
            return False

    async def stop_all_bridges(self):
        """Best-effort cleanup for all running bridges."""
        for server_id in list(self.bridges.keys()):
            await self.stop_bridge(server_id)
    
    async def get_mcp_manifest(self, server_id: str) -> Optional[Dict[str, Any]]:
        """
        Get the manifest for an MCP server.
        
        Args:
            server_id: The MCP server ID (agent_name)
            
        Returns:
            Manifest dict or None if not found
        """
        manifest_path = self._resolve_manifest_path(server_id)
        if manifest_path is None:
            return None

        try:
            with open(manifest_path, "r", encoding="utf-8") as f:
                return json.load(f)
        except Exception as e:
            self.logger.error(f"Failed to read manifest for {server_id}: {e}")

        return None
    
    async def call_mcp_tool(
        self, server_id: str, tool_name: str, arguments: Dict[str, Any]
    ) -> ToolCallResponse:
        """
        Call an MCP tool via HTTP bridge.
        
        Args:
            server_id: The MCP server ID (agent_name)
            tool_name: Name of the tool to call
            arguments: Tool arguments
            
        Returns:
            ToolCallResponse with result or error
        """
        try:
            self.logger.info(f"Routing MCP tool call: {server_id}/{tool_name}")
            bridge = await self._get_or_start_bridge(server_id)
            bridge_response = await bridge.call_tool(tool_name, arguments)
            return ToolCallResponse(
                success=bridge_response.success,
                result=bridge_response.result,
                error=bridge_response.error,
            )
        except FileNotFoundError as e:
            return ToolCallResponse(success=False, error=str(e))
        except Exception as e:
            self.logger.error(f"Failed to call MCP tool: {e}")
            return ToolCallResponse(success=False, error=str(e))
    
    async def list_mcp_servers(self) -> Dict[str, Any]:
        """
        List all available MCP servers.
        
        Returns:
            Dict with server metadata
        """
        servers = {}
        
        try:
            if self.agents_directory.exists():
                for server_dir in self.agents_directory.iterdir():
                    if server_dir.is_dir():
                        manifest_path = self._resolve_manifest_path(server_dir.name)
                        if manifest_path is None:
                            continue

                        try:
                            with open(manifest_path, "r", encoding="utf-8") as f:
                                manifest = json.load(f)
                                servers[server_dir.name] = {
                                    "name": manifest.get("name"),
                                    "version": manifest.get("version"),
                                    "tools": len(manifest.get("tools", [])),
                                    "resources": len(manifest.get("resources", [])),
                                    "prompts": len(manifest.get("prompts", [])),
                                    "bridge_url": f"/mcp/{server_dir.name}/tool",
                                    "bridge_running": server_dir.name in self.bridges,
                                }
                        except Exception as e:
                            self.logger.warning(
                                f"Failed to read manifest for {server_dir.name}: {e}"
                            )
        
        except Exception as e:
            self.logger.error(f"Failed to list MCP servers: {e}")
        
        return servers
