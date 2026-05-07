"""
Nasiko-Compliant Gateway Agent
Track 2: LLM Router Gateway Integration

Uses a virtual key + gateway URL instead of hardcoded provider keys.
Nasiko agent structure contract:
  nasiko-agent/
  ├── docker-compose.yml
  ├── Dockerfile
  └── src/
      └── main.py
"""

import os
import time
from typing import Optional, Dict, Any, List
from fastapi import FastAPI, HTTPException
from fastapi.middleware.cors import CORSMiddleware
from pydantic import BaseModel
import httpx

app = FastAPI(
    title="Gateway Demo Agent",
    description="Nasiko agent using LLM Gateway (Track 2)",
    version="1.0.0"
)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

# ── Gateway config injected at runtime — zero hardcoded keys ─────────────────
GATEWAY_URL = os.getenv("GATEWAY_URL", "http://litellm-gateway:4000")
GATEWAY_VIRTUAL_KEY = os.getenv("GATEWAY_VIRTUAL_KEY", "")
DEFAULT_MODEL = os.getenv("LLM_MODEL", "gemini-flash")

# ── Models ────────────────────────────────────────────────────────────────────

class ChatRequest(BaseModel):
    message: str
    model: Optional[str] = None
    system_prompt: Optional[str] = "You are a helpful assistant."
    max_tokens: Optional[int] = 500
    temperature: Optional[float] = 0.7

class ChatResponse(BaseModel):
    response: str
    model_used: str
    provider: str
    tokens_used: int
    gateway_url: str
    virtual_key_prefix: str

# ── Gateway call ──────────────────────────────────────────────────────────────

def call_gateway(messages: List[Dict], model: Optional[str] = None,
                 max_tokens: int = 500, temperature: float = 0.7) -> Dict[str, Any]:
    if not GATEWAY_VIRTUAL_KEY:
        raise RuntimeError(
            "GATEWAY_VIRTUAL_KEY not set. Run: make generate-key team=demo"
        )

    payload = {
        "model": model or DEFAULT_MODEL,
        "messages": messages,
        "max_tokens": max_tokens,
        "temperature": temperature,
    }
    headers = {
        "Authorization": f"Bearer {GATEWAY_VIRTUAL_KEY}",
        "Content-Type": "application/json",
    }

    last_err = None
    for attempt in range(3):
        try:
            with httpx.Client(timeout=60.0) as client:
                resp = client.post(f"{GATEWAY_URL}/chat/completions",
                                   json=payload, headers=headers)
                resp.raise_for_status()
                return resp.json()
        except httpx.HTTPStatusError as e:
            if e.response.status_code in (401, 403):
                raise RuntimeError("Gateway auth error: check GATEWAY_VIRTUAL_KEY")
            last_err = e
        except Exception as e:
            last_err = e
        if attempt < 2:
            time.sleep(1.5 * (attempt + 1))

    raise RuntimeError(f"Gateway call failed after 3 attempts: {last_err}")

# ── Endpoints ─────────────────────────────────────────────────────────────────

@app.get("/health")
async def health():
    gw_ok = False
    try:
        with httpx.Client(timeout=5.0) as c:
            r = c.get(f"{GATEWAY_URL}/health")
            gw_ok = r.status_code == 200
    except Exception:
        pass
    return {
        "status": "healthy",
        "agent": "gateway-demo-agent",
        "gateway_url": GATEWAY_URL,
        "gateway_reachable": gw_ok,
        "virtual_key_set": bool(GATEWAY_VIRTUAL_KEY),
        "default_model": DEFAULT_MODEL,
    }

@app.post("/chat", response_model=ChatResponse)
async def chat(request: ChatRequest):
    messages = [
        {"role": "system", "content": request.system_prompt},
        {"role": "user", "content": request.message},
    ]
    try:
        result = call_gateway(messages=messages, model=request.model,
                              max_tokens=request.max_tokens,
                              temperature=request.temperature)
    except Exception as e:
        raise HTTPException(status_code=502, detail=str(e))

    content = result["choices"][0]["message"]["content"]
    model_used = result.get("model", DEFAULT_MODEL)
    usage = result.get("usage", {})

    provider_map = {
        "gemini": "Google Gemini",
        "gpt": "OpenAI",
        "claude": "Anthropic",
        "groq": "Groq",          # fixed: was "xAI Grok"
        "llama": "Groq",
        "mixtral": "Groq",
    }
    provider = next(
        (v for k, v in provider_map.items() if k in model_used.lower()), "Gateway"
    )

    return ChatResponse(
        response=content,
        model_used=model_used,
        provider=provider,
        tokens_used=usage.get("total_tokens", 0),
        gateway_url=GATEWAY_URL,
        virtual_key_prefix=GATEWAY_VIRTUAL_KEY[:12] + "..." if GATEWAY_VIRTUAL_KEY else "not-set",
    )

@app.post("/analyze")
async def analyze(request: ChatRequest):
    """Alias for /chat — Nasiko routing compatibility."""
    return await chat(request)

@app.get("/capabilities")
async def capabilities():
    return {
        "name": "gateway-demo-agent",
        "description": "Provider-agnostic chat agent via LiteLLM gateway",
        "capabilities": ["chat", "question_answering", "text_generation"],
        "input_types": ["text"],
        "output_types": ["text"],
        "endpoints": {
            "/chat": "Chat with LLM via gateway",
            "/analyze": "Analyze text via gateway",
            "/health": "Health check",
        },
        "gateway_pattern": True,
        "hardcoded_keys": False,
    }
