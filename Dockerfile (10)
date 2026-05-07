"""
MCP Calculator Agent - A2A HTTP agent that performs math operations.
Works with the Nasiko web UI chat interface.
"""
import re
import logging
import uvicorn
from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import JSONResponse
from starlette.routing import Route

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("calculator-agent")


def evaluate_math(query: str) -> str:
    """Parse a math question and return the answer."""
    q = query.lower().strip()

    # Extract numbers from the query
    numbers = [float(x) for x in re.findall(r'-?\d+\.?\d*', q)]

    if len(numbers) < 2:
        return f"Please provide at least two numbers. I found: {numbers}"

    a, b = numbers[0], numbers[1]

    if any(word in q for word in ["add", "plus", "sum", "+"]):
        result = a + b
        return f"{a} + {b} = {result}"
    elif any(word in q for word in ["subtract", "minus", "difference", "-"]):
        result = a - b
        return f"{a} - {b} = {result}"
    elif any(word in q for word in ["multiply", "times", "product", "*", "x"]):
        result = a * b
        return f"{a} × {b} = {result}"
    elif any(word in q for word in ["divide", "divided", "quotient", "/"]):
        if b == 0:
            return "Error: Cannot divide by zero!"
        result = a / b
        return f"{a} ÷ {b} = {result}"
    elif any(word in q for word in ["power", "exponent", "^", "**"]):
        result = a ** b
        return f"{a} ^ {b} = {result}"
    else:
        # Default: try addition
        result = a + b
        return f"I'll add them: {a} + {b} = {result}"


# ---- A2A JSON-RPC Protocol Handlers ----

async def handle_jsonrpc(request: Request) -> JSONResponse:
    """Handle A2A JSON-RPC requests from the Nasiko platform."""
    try:
        body = await request.json()
    except Exception:
        return JSONResponse({"error": "Invalid JSON"}, status_code=400)

    method = body.get("method", "")
    req_id = body.get("id", 1)
    params = body.get("params", {})

    logger.info(f"Received method: {method}")

    if method == "agent/info":
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "name": "MCP Calculator Agent",
                "description": "A calculator agent that performs math operations",
                "version": "1.0.0",
                "capabilities": {"streaming": False},
                "skills": [
                    {"id": "calculator", "name": "Calculator", "description": "Perform math operations"}
                ]
            }
        })

    elif method in ("message/send", "tasks/send"):
        # Extract the user's message
        message = params.get("message", {})
        parts = message.get("parts", [])
        user_text = ""
        for part in parts:
            # Platform uses "kind": "text", some clients use "type": "text"
            if part.get("kind") == "text" or part.get("type") == "text" or "text" in part:
                user_text = part.get("text", str(part))
                break

        if not user_text:
            user_text = str(params)

        # Calculate the answer
        answer = evaluate_math(user_text)
        logger.info(f"Query: {user_text} -> Answer: {answer}")

        import uuid as _uuid
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "kind": "message",
                "role": "agent",
                "messageId": str(_uuid.uuid4()),
                "parts": [{"kind": "text", "text": answer}],
            }
        })

    elif method == "tasks/get":
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": {
                "id": params.get("id", "task-1"),
                "status": {"state": "completed"}
            }
        })

    else:
        return JSONResponse({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": {"code": -32601, "message": f"Method not found: {method}"}
        })


async def health(request: Request) -> JSONResponse:
    return JSONResponse({"status": "ok"})


async def agent_card(request: Request) -> JSONResponse:
    """Return AgentCard at root URL (what Kong checks)."""
    return JSONResponse({
        "name": "MCP Calculator Agent",
        "description": "A calculator agent that can add, subtract, multiply, and divide numbers. Built for the Nasiko MCP Hackathon.",
        "version": "1.0.0",
        "url": "http://localhost:5000/",
        "capabilities": {"streaming": False, "pushNotifications": False},
        "skills": [
            {"id": "add", "name": "Add", "description": "Add two numbers"},
            {"id": "subtract", "name": "Subtract", "description": "Subtract two numbers"},
            {"id": "multiply", "name": "Multiply", "description": "Multiply two numbers"},
            {"id": "divide", "name": "Divide", "description": "Divide two numbers"}
        ],
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"]
    })


app = Starlette(routes=[
    Route("/", agent_card, methods=["GET"]),
    Route("/", handle_jsonrpc, methods=["POST"]),
    Route("/health", health, methods=["GET"]),
])

if __name__ == "__main__":
    logger.info("Starting MCP Calculator Agent on port 5000...")
    uvicorn.run(app, host="0.0.0.0", port=5000)
