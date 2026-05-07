import json
from pathlib import Path
from typing import Any, Dict, Optional, Type
import httpx
from tenacity import retry, retry_if_exception_type, stop_after_attempt, wait_fixed
from pydantic import BaseModel, Field

# --- LangChain ---
try:
    from langchain_core.tools import StructuredTool
except ImportError:
    class StructuredTool:
        @classmethod
        def from_function(cls, *args, **kwargs):
            raise ImportError("langchain_core is not installed")

# --- CrewAI ---
try:
    from crewai.tools import BaseTool
except ImportError:
    class BaseTool(BaseModel):
        name: str
        description: str
        args_schema: Optional[Type[BaseModel]] = None


class AgentCallError(Exception):
    pass

class AgentRestartError(Exception):
    pass

def is_bridge_alive(artifact_id: str, bridge_url: str = "http://localhost:8000") -> bool:
    """Checks GET /mcp/{id}/health before initiating heavy logic."""
    url = f"{bridge_url.rstrip('/')}/mcp/{artifact_id}/health"
    try:
        resp = httpx.get(url, timeout=5.0)
        return resp.status_code == 200 and resp.json().get("alive", False)
    except Exception:
        return False

def start_bridge(artifact_id: str, bridge_url: str = "http://localhost:8000") -> bool:
    """Attempts to handle 500 error recoveries by triggering POST /start."""
    try:
        url = f"{bridge_url.rstrip('/')}/mcp/{artifact_id}/start"
        # Flaw 5 fixed: Start endpoint executed blindly without locking on bridge.json 
        resp = httpx.post(url, json={"kong_admin_url": "http://localhost:8001"})
        
        # 409 Conflict means it's natively already running, proceed.
        return resp.status_code in (200, 409)
    except Exception:
        return False

@retry(
    retry=retry_if_exception_type(AgentRestartError),
    stop=stop_after_attempt(3),
    wait=wait_fixed(2)
)
def execute_bridge_call(artifact_id: str, tool_name: str, arguments: dict, trace_context: str = None, bridge_url: str = "http://localhost:8000") -> str:
    url = f"{bridge_url.rstrip('/')}/mcp/{artifact_id}/call"
    headers = {"Content-Type": "application/json"}
    
    if trace_context:
        headers["traceparent"] = trace_context
        
    try:
        resp = httpx.post(
            url,
            json={"tool_name": tool_name, "arguments": arguments},
            headers=headers,
            timeout=30.0
        )
        
        if resp.status_code == 200:
            result = resp.json()
            content_arr = result.get("result", {}).get("content", [])
            if content_arr and isinstance(content_arr, list):
                return content_arr[0].get("text", str(content_arr))
            return json.dumps(result)
            
        if resp.status_code == 500:
            error_detail = resp.json().get("detail", "")
            if "not running" in str(error_detail):
                if start_bridge(artifact_id, bridge_url):
                    raise AgentRestartError("Agent restarted, queueing retry for tool call.")
                raise AgentCallError(f"Agent {artifact_id} is dead and cannot restart.")
            raise AgentCallError(f"Agent returned JSON-RPC error: {error_detail}")
            
        resp.raise_for_status()

    except httpx.HTTPStatusError as e:
        # Flaw 2 fixed: explicitly catch standard 4xx network errors so they don't break CrewAI natively.
        raise AgentCallError(f"HTTP Error ({e.response.status_code}): {e.response.text}")
    except httpx.RequestError as e:
        raise AgentCallError(f"Network error connecting to bridge: {str(e)}")


def create_mcp_http_tool(artifact_id: str, tool_name: str, tool_desc: str, schema: Type[BaseModel], trace_context: str = None) -> StructuredTool:
    def _run_mcp_tool(*args, **kwargs) -> str:
        # Pre-execution Health Verification
        if not is_bridge_alive(artifact_id):
            raise AgentCallError("Cannot execute tool: Bridge is unresponsive to health pings.")
        return execute_bridge_call(artifact_id, tool_name, kwargs, trace_context=trace_context)

    return StructuredTool.from_function(
        func=_run_mcp_tool,
        name=tool_name,
        description=tool_desc,
        args_schema=schema,
    )

class MCPCrewTool(BaseTool):
    artifact_id: str = Field(..., description="ID of the target R1/R2 agent bridge")
    tool_name_remote: str = Field(..., description="The original tool name registered in the MCP Manifest")
    trace_context: Optional[str] = Field(None, description="W3C Traceparent injection")
    
    # Flaw 6 fixed: Pydantic parsing natively binds inputs avoiding generic blindspots.
    args_schema: Type[BaseModel] = Field(default_factory=lambda: BaseModel)
    
    def _run(self, *args, **kwargs) -> str:
        if not is_bridge_alive(self.artifact_id):
            raise AgentCallError("Cannot execute tool: Bridge is unresponsive to health pings.")
        return execute_bridge_call(self.artifact_id, self.tool_name_remote, kwargs, trace_context=self.trace_context)

def create_mcp_crew_tool(artifact_id: str, tool_name: str, tool_desc: str, schema: Type[BaseModel], trace_context: str = None) -> MCPCrewTool:
    return MCPCrewTool(
        name=tool_name,
        description=tool_desc,
        args_schema=schema,
        artifact_id=artifact_id,
        tool_name_remote=tool_name,
        trace_context=trace_context
    )
