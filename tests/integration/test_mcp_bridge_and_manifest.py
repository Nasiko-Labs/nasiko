import asyncio
import json
import os
import shutil
import httpx
import pytest
from pathlib import Path
from typing import Dict, Any

from app.utils.agentcard_generator.mcp_manifest_generator import McpManifestGeneratorAgent
from orchestrator.mcp_bridge import app
import uvicorn

# Mock data for McpManifestGeneratorAgent
MOCK_FAST_MCP_SOURCE = """
from mcp.server.fastmcp import FastMCP
mcp = FastMCP("weather-mcp")

@mcp.tool()
def get_weather(location: str) -> str:
    \"\"\"Returns the weather for a given city\"\"\"
    return f"Weather for {location} is Sunny"

if __name__ == "__main__":
    mcp.run()
"""

@pytest.fixture
def mock_mcp_project(tmp_path):
    project_dir = tmp_path / "weather_mcp"
    project_dir.mkdir()
    src_dir = project_dir / "src"
    src_dir.mkdir()
    (src_dir / "main.py").write_text(MOCK_FAST_MCP_SOURCE)
    (project_dir / "Dockerfile").write_text("FROM python:3.10-slim\n")
    return project_dir

@pytest.mark.asyncio
async def test_mcp_manifest_generation_accuracy(mock_mcp_project):
    # We use the fallback generation if no API key is set for reasoning, 
    # but here we'll test the core logic.
    # Note: For real LLM tests we'd need keys, but we can verify the AgentAnalyzerTools 
    # which the Generator uses to find files.
    
    agent = McpManifestGeneratorAgent(api_key="dummy")
    # Stubbing the actual LLM call to verify it gets the right inputs would be complex, 
    # instead we verify the tool discovery part of the system.
    
    tools = agent.tools
    result = tools.glob_files("**/*.py", str(mock_mcp_project))
    assert result["status"] == "success"
    files = result["files"]
    assert any("main.py" in f for f in files)
    
    result = tools.read_file(str(mock_mcp_project / "src" / "main.py"))
    assert result["status"] == "success"
    code = result["content"]
    assert "FastMCP" in code
    assert "@mcp.tool()" in code

@pytest.mark.asyncio
async def test_mcp_manifest_content_structure(mock_mcp_project):
    # Test the mock tool in McpManifestGeneratorAgent that produces the final JSON
    agent = McpManifestGeneratorAgent(api_key="dummy")
    tools = agent.tools
    
    result = tools.generate_mcp_manifest_json(
        agent_name="weather-mcp",
        description="A weather server",
        version="1.0.0",
        tools=[{"name": "get_weather", "description": "Get weather info"}],
        resources=[]
    )
    
    assert result["status"] == "success"
    manifest = result["agentcard"]
    assert manifest["artifact_type"] == "mcp_server"
    assert manifest["name"] == "weather-mcp"
    assert manifest["capabilities"]["tools"][0]["name"] == "get_weather"

@pytest.mark.asyncio
async def test_mcp_bridge_startup_and_tool_call(mock_mcp_project):
    # Set environment variables for the bridge
    os.environ["MCP_SERVER_SCRIPT"] = str(mock_mcp_project / "src" / "main.py")
    
    # We use a context manager to start/stop the bridge and its internal subprocess
    from contextlib import asynccontextmanager
    
    # Start the bridge server in a separate thread/task
    import uvicorn
    import socket

    def find_free_port():
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.bind(('', 0))
            return s.getsockname()[1]

    port = find_free_port()
    
    config = uvicorn.Config(app, host="127.0.0.1", port=port, log_level="warning")
    server = uvicorn.Server(config)
    
    task = asyncio.create_task(server.serve())
    
    # Give it a second to initialize the MCP session
    await asyncio.sleep(3)
    
    try:
        async with httpx.AsyncClient(base_url=f"http://127.0.0.1:{port}") as client:
            # Test health
            resp = await client.get("/health")
            assert resp.status_code == 200
            assert resp.json()["mcp_connected"] is True
            
            # Test list tools
            resp = await client.get("/tools")
            assert resp.status_code == 200
            tools = resp.json()["tools"]
            assert any(t["name"] == "get_weather" for t in tools)
            
            # Test call tool
            resp = await client.post("/tools/call", json={
                "name": "get_weather",
                "arguments": {"location": "London"}
            })
            assert resp.status_code == 200
            json_resp = resp.json()
            assert json_resp["status"] == "success"
            # FastMCP format check
            # mcp bridge uses c.model_dump() on contents
            assert "Sunny" in str(json_resp["content"])
            
    finally:
        server.should_exit = True
        await task

