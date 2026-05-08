"""
Gateway LLM Agent — example agent that routes all LLM calls through the LiteLLM gateway.

No provider API keys are stored here. The orchestrator injects:
  OPENAI_BASE_URL=http://litellm:4000/v1
  OPENAI_API_KEY=virtual-key (or per-agent minted key)

Tracing: every /chat request produces:
  Agent Request Span
    └─► gateway_span (llm.completion)
          └─► LiteLLM provider span  ← exported by LiteLLM to Phoenix
"""

import os
import uvicorn
from fastapi import FastAPI
from pydantic import BaseModel
from openai import OpenAI

# Bootstrap Phoenix tracing so this agent's spans have a parent trace.
# openinference.instrumentation.openai automatically injects traceparent headers
# into every OpenAI SDK call, linking gateway spans back to this trace.
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry import trace
from opentelemetry.sdk.resources import Resource

PHOENIX_ENDPOINT = os.getenv("PHOENIX_COLLECTOR_ENDPOINT", "http://phoenix-observability:6006/v1/traces")
AGENT_NAME = os.getenv("AGENT_NAME", "gateway-llm-agent")

def _setup_tracing():
    resource = Resource.create({"service.name": AGENT_NAME})
    provider = TracerProvider(resource=resource)
    provider.add_span_processor(
        BatchSpanProcessor(OTLPSpanExporter(endpoint=PHOENIX_ENDPOINT))
    )
    trace.set_tracer_provider(provider)

    # Instrument the OpenAI SDK — works for ALL providers (Groq, MiniMax, etc.)
    # because agents always call LiteLLM via the OpenAI-compatible interface.
    # The instrumentor injects traceparent headers into the HTTP request to LiteLLM,
    # enabling parent-child span linking in Phoenix regardless of the upstream provider.
    try:
        from openinference.instrumentation.openai import OpenAIInstrumentor
        OpenAIInstrumentor().instrument(tracer_provider=provider)
    except ImportError:
        pass

    # Fallback: instrument all outgoing HTTP calls (covers any SDK, any provider)
    try:
        from opentelemetry.instrumentation.httpx import HTTPXClientInstrumentor
        HTTPXClientInstrumentor().instrument()
    except ImportError:
        pass

_setup_tracing()

app = FastAPI(title="Gateway LLM Agent")

# Client reads OPENAI_BASE_URL and OPENAI_API_KEY from env — both injected by orchestrator.
client = OpenAI(
    base_url=os.getenv("OPENAI_BASE_URL", "http://litellm:4000/v1"),
    api_key=os.getenv("OPENAI_API_KEY", "virtual-key"),
)

tracer = trace.get_tracer(AGENT_NAME)


class ChatRequest(BaseModel):
    message: str
    model: str = "llama3-fast"


@app.get("/health")
def health():
    return {"status": "ok"}


@app.post("/chat")
def chat(request: ChatRequest):
    # Start an agent-level span. The instrumented OpenAI SDK will create a
    # child span for the LiteLLM call and inject traceparent into the HTTP
    # request so LiteLLM's OTEL span links back here in Phoenix.
    with tracer.start_as_current_span("agent.chat") as span:
        span.set_attribute("agent.name", AGENT_NAME)
        span.set_attribute("llm.model", request.model)
        span.set_attribute("input.message", request.message)

        response = client.chat.completions.create(
            model=request.model,
            messages=[{"role": "user", "content": request.message}],
        )
        reply = response.choices[0].message.content
        span.set_attribute("output.reply", reply)
        return {"reply": reply}


if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=8000)
