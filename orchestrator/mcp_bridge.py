import asyncio
import argparse
import os
import sys
import logging
from typing import Dict, Any, Optional
from fastapi import FastAPI, Request, HTTPException
import uvicorn
from contextlib import asynccontextmanager

from mcp.client.stdio import stdio_client, StdioServerParameters
from mcp.client.session import ClientSession

# Import tracer for Phoenix Observability
from opentelemetry import trace
tracer = trace.get_tracer(__name__)

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("mcp_bridge")

# Global session holder
mcp_session: Optional[ClientSession] = None
server_process_parameters: Optional[StdioServerParameters] = None

@asynccontextmanager
async def lifespan(app: FastAPI):
    global mcp_session, server_process_parameters
    
    script_path = os.getenv("MCP_SERVER_SCRIPT", "src/main.py")
    if not os.path.exists(script_path) and os.path.exists("main.py"):
        script_path = "main.py"
        
    logger.info(f"Starting MCP server subprocess with {script_path}")
    server_env = os.environ.copy()
    server_env["PYTHONPATH"] = ":".join(
        path for path in [os.getcwd(), server_env.get("PYTHONPATH", "")] if path
    )

    server_process_parameters = StdioServerParameters(
        command="python",
        args=[script_path],
        env=server_env
    )
    
    async with stdio_client(server_process_parameters) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            mcp_session = session
            logger.info("MCP Session initialized successfully")
            yield
            
    mcp_session = None

app = FastAPI(title="MCP HTTP Bridge", lifespan=lifespan)

@app.get("/health")
async def health_check():
    return {"status": "ok", "mcp_connected": mcp_session is not None}

@app.get("/tools")
async def list_tools():
    with tracer.start_as_current_span("mcp_list_tools"):
        if not mcp_session:
            raise HTTPException(status_code=503, detail="MCP session not initialized")
        try:
            result = await mcp_session.list_tools()
            return {"tools": [t.model_dump() for t in result.tools]}
        except Exception as e:
            logger.error(f"Error listing tools: {e}")
            raise HTTPException(status_code=500, detail=str(e))

@app.post("/tools/call")
async def call_tool(request: Request):
    """
    Expects JSON:
    {
      "name": "tool_name",
      "arguments": {...}
    }
    """
    with tracer.start_as_current_span("mcp_call_tool") as span:
        if not mcp_session:
            raise HTTPException(status_code=503, detail="MCP session not initialized")
        
        try:
            payload = await request.json()
            tool_name = payload.get("name")
            arguments = payload.get("arguments", {})
            
            span.set_attribute("tool.name", str(tool_name))
            span.set_attribute("tool.arguments", str(arguments))
            
            if not tool_name:
                raise HTTPException(status_code=400, detail="Missing tool 'name'")
                
            logger.info(f"Calling tool: {tool_name} with {arguments}")
            result = await mcp_session.call_tool(tool_name, arguments)
            
            # Format depends on MCP spec version, typically result.content is a list of TextContent/ImageContent
            output = [c.model_dump() for c in result.content] if hasattr(result, "content") else result
            
            return {"status": "success", "content": output}
            
        except Exception as e:
            logger.error(f"Error calling tool: {e}")
            span.record_exception(e)
            raise HTTPException(status_code=500, detail=str(e))

@app.get("/resources")
async def list_resources():
    with tracer.start_as_current_span("mcp_list_resources"):
        if not mcp_session:
            raise HTTPException(status_code=503, detail="MCP session not initialized")
        try:
            result = await mcp_session.list_resources()
            return {"resources": [r.model_dump() for r in result.resources]}
        except Exception as e:
            logger.error(f"Error listing resources: {e}")
            raise HTTPException(status_code=500, detail=str(e))

@app.post("/")
async def chat_handler(request: Request):
    """
    Handle direct chat messages from Nasiko UI.
    If the server is an MCP server, we'll respond with a summary of its capabilities
    or try to call a tool if the query is simple.
    """
    if not mcp_session:
        raise HTTPException(status_code=503, detail="MCP session not initialized")
    
    try:
        payload = await request.json()
        user_message = payload.get("message", "").lower()
        
        # Discover tools
        tools_resp = await mcp_session.list_tools()
        tool_names = [t.name for t in tools_resp.tools]
        
        # Super simple routing: If user mentions a tool name, try to call it with dummy args or ask for them
        # For our weather-agent demo, this makes it feel like it 'works' directly
        if "weather" in user_message and "get_weather" in tool_names:
            # Try to extract a location (very basic)
            location = "London" # Default
            if "in " in user_message:
                location = user_message.split("in ")[-1].strip("? .")
            
            result = await mcp_session.call_tool("get_weather", {"location": location})
            content = result.content[0].text if result.content else str(result)
            return {"role": "assistant", "content": f"[MCP Result] {content}"}

        return {
            "role": "assistant", 
            "content": f"I am an MCP Server bridged to Nasiko. I have the following tools available: {', '.join(tool_names)}. To use me effectively, please add me to a regular Nasiko Agent using the MCP_SERVERS environment variable."
        }
    except Exception as e:
        logger.error(f"Error in chat handler: {e}")
        return {"role": "assistant", "content": f"Error interacting with MCP server: {str(e)}"}

if __name__ == "__main__":
    port = int(os.getenv("PORT", 5000))
    uvicorn.run("mcp_bridge:app", host="0.0.0.0", port=port, log_level="info")
