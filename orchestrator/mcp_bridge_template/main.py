import os
import sys
import json
import uuid
import asyncio
import logging
from typing import Dict, Any
from fastapi import FastAPI, HTTPException, Request

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

app = FastAPI(title="MCP HTTP Bridge")

class MCPBridgeProcess:
    def __init__(self, script_path: str):
        self.script_path = script_path
        self.process = None
        self.pending_requests: Dict[str, asyncio.Future] = {}
        self.lock = asyncio.Lock()

    async def start(self):
        logger.info(f"Starting MCP Process: python {self.script_path}")
        self.process = await asyncio.create_subprocess_exec(
            sys.executable, self.script_path,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE
        )
        # Start background reader task
        asyncio.create_task(self._read_stdout())
        asyncio.create_task(self._read_stderr())

    async def _read_stdout(self):
        while True:
            line = await self.process.stdout.readline()
            if not line:
                break
            try:
                msg = json.loads(line.decode().strip())
                msg_id = msg.get("id")
                if msg_id and msg_id in self.pending_requests:
                    self.pending_requests[msg_id].set_result(msg)
            except json.JSONDecodeError:
                # Normal stdout prints that aren't JSONRPC are ignored or logged
                pass
            except Exception as e:
                logger.error(f"Error reading stdout: {e}")

    async def _read_stderr(self):
        while True:
            line = await self.process.stderr.readline()
            if not line:
                break
            logger.warning(f"MCP STDERR: {line.decode().strip()}")

    async def call_tool(self, tool_name: str, arguments: Dict[str, Any]) -> Dict[str, Any]:
        if not self.process:
            await self.start()
            
        req_id = str(uuid.uuid4())
        payload = {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {
                "name": tool_name,
                "arguments": arguments
            }
        }
        
        future = asyncio.get_event_loop().create_future()
        self.pending_requests[req_id] = future
        
        payload_bytes = json.dumps(payload).encode() + b"\n"
        
        async with self.lock:
            self.process.stdin.write(payload_bytes)
            await self.process.stdin.drain()
            
        try:
            # Wait with timeout
            response = await asyncio.wait_for(future, timeout=30.0)
            if "error" in response:
                raise HTTPException(status_code=400, detail=response["error"])
            return response.get("result", {})
        except asyncio.TimeoutError:
            del self.pending_requests[req_id]
            raise HTTPException(status_code=504, detail="MCP Server timeout")
        except Exception as e:
            if req_id in self.pending_requests:
                del self.pending_requests[req_id]
            raise e

bridge = MCPBridgeProcess(os.environ.get("MCP_SCRIPT", "agent.py"))

@app.on_event("startup")
async def startup_event():
    await bridge.start()

@app.post("/invoke/{tool_name}")
async def invoke_tool(tool_name: str, request: Request):
    """Bridge endpoint for agents calling an MCP tool"""
    try:
        body = await request.json()
    except Exception:
        body = {}
    
    arguments = body.get("arguments", body)
    
    logger.info(f"Invoking {tool_name} with {arguments}")
    result = await bridge.call_tool(tool_name, arguments)
    return result

@app.get("/health")
def health():
    return {"status": "ok", "type": "mcp_bridge"}

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8000)
