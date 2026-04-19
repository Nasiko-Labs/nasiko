"""
Nasiko MCP Agent Patch
This script is intended to be injected by the orchestrator into standard Python agents
(like LangChain or CrewAI) AT RUNTIME without requiring developers to change their code.

It dynamically scans the Nasiko Kong API (or registry) to discover published MCP Tools,
and exposes them as native Langchain `@tool` decorated functions for the agent's LLM to consume.
"""

import os
import urllib.parse
from typing import List, Any
import requests

try:
    from langchain.tools import tool
except ImportError:
    tool = None

NASIKO_KONG_URL = os.environ.get("KONG_GATEWAY_URL", "http://kong:8000")

def load_mcp_tools(agent_registry_id: str) -> List[Any]:
    """
    Fetches the MCP manifest via Nasiko Registry, and builds Python functions.
    If LangChain is present, they are wrapped as native @tools.
    """
    if not tool:
        return []
        
    try:
        # 1. Fetch manifest from Kong gateway (or registry)
        res = requests.get(f"{NASIKO_KONG_URL}/api/v1/registry/agent/{agent_registry_id}")
        if res.status_code != 200:
            return []
            
        data = res.json()
        if data.get("artifactType") != "mcp_server":
            return []
            
        mcp_tools = []
        
        # 2. Iteratively create @tool wrappers pointing to the HTTP bridge
        for t in data.get("tools", []):
            name = t.get("name")
            desc = t.get("description", "")
            
            # Create a closure
            def mcp_caller(**kwargs):
                bridge_url = f"{NASIKO_KONG_URL}/agents/{agent_registry_id}/invoke/{urllib.parse.quote(name)}"
                post_res = requests.post(bridge_url, json={"arguments": kwargs})
                if post_res.status_code != 200:
                    return f"Error from MCP: {post_res.text}"
                return post_res.json()
                
            # Name and docstring are critical for LLMs
            mcp_caller.__name__ = name
            mcp_caller.__doc__ = desc
            
            # Wrap as Langchain tool
            mcp_tools.append(tool(mcp_caller))
            
        return mcp_tools
        
    except Exception as e:
        import logging
        logging.getLogger(__name__).error(f"Failed to load MCP tools: {e}")
        return []

def auto_inject_mcp_to_langchain():
    """
    This is called by the injector on agent startup. 
    It can monkeypatch common tool retrieval mechanics if needed, or simply populate a global.
    """
    mcp_servers = os.environ.get("LINKED_MCP_SERVERS", "").split(",")
    all_tools = []
    for srv in mcp_servers:
        if srv:
            all_tools.extend(load_mcp_tools(srv.strip()))
    
    # Exposing them into the global namespace or a predefined module
    import builtins
    builtins.NASIKO_MCP_TOOLS = all_tools
