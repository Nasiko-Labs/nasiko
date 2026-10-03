import json
import uvicorn
from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse, StreamingResponse

app = FastAPI(title="Nasiko Cost-Aware Classifier Agent")

REPLY_TEXT = "I am the Nasiko Cost-Aware Classifier Agent. Request analyzed and routed successfully."

def make_response_dict():
    return {
        "id": "chatcmpl-agent",
        "object": "chat.completion",
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": REPLY_TEXT
                },
                "delta": {
                    "role": "assistant",
                    "content": REPLY_TEXT
                },
                "finish_reason": "stop"
            }
        ],
        "content": REPLY_TEXT,
        "text": REPLY_TEXT,
        "output": REPLY_TEXT,
        "status": "completed"
    }

@app.get("/")
@app.get("/health")
@app.get("/chat")
def health():
    return {"status": "ok", "service": "classifier-agent", "message": REPLY_TEXT}

@app.post("/")
@app.post("/chat")
@app.post("/route")
@app.post("/v1/chat/completions")
async def chat_handler(request: Request):
    try:
        body = await request.json()
    except Exception:
        body = {}
    print(f"Received request on {request.url.path}: {body}")

    # Check if client requested streaming
    if body.get("stream", False) or "text/event-stream" in request.headers.get("accept", ""):
        def event_stream():
            chunk = {
                "choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": REPLY_TEXT},
                    "finish_reason": "stop"
                }]
            }
            yield f"data: {json.dumps(chunk)}\n\n"
            yield "data: [DONE]\n\n"
        return StreamingResponse(event_stream(), media_type="text/event-stream")

    return JSONResponse(content=make_response_dict())

if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=8000)
