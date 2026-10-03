import os
import uvicorn
from fastapi import FastAPI
from pydantic import BaseModel

app = FastAPI(title="Nasiko Agent", version="1.0.0")

class PromptRequest(BaseModel):
    query: str
    context: str | None = None

@app.get("/health")
def health():
    return {"status": "healthy"}

@app.post("/route")
def route_prompt(req: PromptRequest):
    return {"status": "success", "message": f"Processed: {req.query}"}

if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=8000)