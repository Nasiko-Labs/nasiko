import json
from pathlib import Path
from fastapi import APIRouter, HTTPException
from pydantic import BaseModel

app = APIRouter()

class LinkRequest(BaseModel):
    agent_artifact_id: str
    mcp_artifact_id: str

def get_bridge_status(mcp_artifact_id: str) -> str:
    path = Path(f"/tmp/nasiko/{mcp_artifact_id}/bridge.json")
    if not path.exists():
        return "UNKNOWN"
    try:
        with open(path, "r") as f:
            data = json.load(f)
            return data.get("status", "UNKNOWN")
    except Exception:
        return "ERROR"

def get_manifest(mcp_artifact_id: str) -> dict:
    path = Path(f"/tmp/nasiko/{mcp_artifact_id}/manifest.json")
    if not path.exists():
        raise HTTPException(status_code=404, detail="manifest.json not found")
    try:
        with open(path, "r") as f:
            return json.load(f)
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Failed to read manifest: {e}")

@app.post("/link")
def link_agent_to_mcp(req: LinkRequest):
    """
    Priority 3: Binds an orchestrating agent to an MCP child agent.
    Must verify the target bridge status is 'ready' before proceeding.
    """
    status = get_bridge_status(req.mcp_artifact_id)
    if status != "ready": 
        raise HTTPException(status_code=400, detail=f"Cannot link. MCP Bridge status is {status}, expected ready.")

    manifest = get_manifest(req.mcp_artifact_id)
    
    return {
        "status": "success",
        "linked_mcp": req.mcp_artifact_id,
        "parent_agent": req.agent_artifact_id,
        "available_tools": [t.get("name") for t in manifest.get("tools", [])]
    }
