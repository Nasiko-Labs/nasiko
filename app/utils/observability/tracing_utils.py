
import os
import json
import logging
import importlib
from contextvars import ContextVar
from typing import Optional

from opentelemetry import trace
from opentelemetry.sdk.trace import SpanProcessor
from opentelemetry.sdk.trace.export import SimpleSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from phoenix.otel import register

# Initialize Logger & Context
logger = logging.getLogger("observability")
session_id_ctx = ContextVar("session_id", default=None)

def bootstrap_tracing(
    project_name: str, 
    endpoint: Optional[str] = None,
    instrumentors: Optional[list] = None,
    framework: Optional[str] = None
):
    """
    Sets up Phoenix tracing and auto-instruments the agent at runtime.
    
    Args:
        project_name: Name of the project in Phoenix.
        endpoint: Collector endpoint (overrides environment variable)
        instrumentors: List of instrumentors to apply.
        framework: Agent framework name (e.g., 'langchain', 'crewai', 'autogen', 'llama-index')
    """
    # Get endpoint from environment or parameter
    if endpoint is None:
        endpoint = os.getenv(
            "PHOENIX_COLLECTOR_ENDPOINT", 
            "http://localhost:6006/v1/traces"
        )
    
    # Check if tracing is enabled
    if os.getenv("TRACING_ENABLED", "true").lower() == "false":
        logger.info(f"Tracing disabled for {project_name}")
        return
    
    # Determine instrumentors based on framework
    if instrumentors is None:
        instrumentors = _get_instrumentors_for_framework(framework)

    logger.info(f"Bootstrapping Phoenix Tracing for '{project_name}' -> {endpoint}")

    try:
        # Initialize Tracing Provider
        provider = register(
            project_name=project_name, 
            endpoint=endpoint, 
            auto_instrument=True
        )
        provider.add_span_processor(ContextSessionIdProcessor())
        provider.add_span_processor(SimpleSpanProcessor(OTLPSpanExporter(endpoint=endpoint)))

        # Apply instrumentors
        for instr in instrumentors:
            try:
                instance = instr() if isinstance(instr, type) else instr
                if hasattr(instance, "instrument"):
                    instance.instrument(tracer_provider=provider)
            except Exception as e:
                logger.warning(f"Failed to apply instrumentor {instr}: {e}")

        # Apply hooks
        _patch_uvicorn(project_name)
        
        logger.info(f"✅ Tracing successfully initialized for {project_name}")
        
    except Exception as e:
        logger.error(f"❌ Failed to initialize tracing: {e}")

def _get_instrumentors_for_framework(framework: Optional[str]) -> list:
    """
    Get appropriate instrumentors based on the agent framework.
    
    Args:
        framework: Agent framework name (e.g., 'langchain', 'crewai', 'autogen', 'llama-index')
        
    Returns:
        List of instrumentor classes
    """
    # Try to import instrumentors dynamically to avoid hard dependencies
    def try_import_instrumentor(module_name, class_name):
        try:
            module = importlib.import_module(module_name)
            return getattr(module, class_name)
        except (ImportError, AttributeError) as e:
            logger.warning(f"Failed to import {class_name} from {module_name}: {e}")
            return None
    
    # Framework-to-instrumentor mapping with dynamic imports
    framework_instrumentors = {
        'langchain': [
            ('openinference.instrumentation.langchain', 'LangChainInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ],
        'crewai': [
            ('openinference.instrumentation.crewai', 'CrewAIInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ],
        'autogen': [
            ('openinference.instrumentation.autogen', 'AutogenInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ],
        'llama-index': [
            ('openinference.instrumentation.llama_index', 'LlamaIndexInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ],
        'dspy': [
            ('openinference.instrumentation.dspy', 'DSPyInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ],
        'haystack': [
            ('openinference.instrumentation.haystack', 'HaystackInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ],
        'anthropic': [
            ('openinference.instrumentation.anthropic', 'AnthropicInstrumentor')
        ],
        'pydantic-ai': [
            ('openinference.instrumentation.pydantic_ai', 'PydanticAIInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ],
        'custom': [
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ]
    }
    
    # Get instrumentors for the framework
    if framework and framework.lower() in framework_instrumentors:
        instrumentor_specs = framework_instrumentors[framework.lower()]
    else:
        # Default to LangChain + OpenAI for backward compatibility
        instrumentor_specs = [
            ('openinference.instrumentation.langchain', 'LangChainInstrumentor'),
            ('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        ]
    
    # Import instrumentors
    instrumentors = []
    for module_name, class_name in instrumentor_specs:
        instrumentor_class = try_import_instrumentor(module_name, class_name)
        if instrumentor_class:
            instrumentors.append(instrumentor_class)
    
    # Fallback to OpenAI if no instrumentors found
    if not instrumentors:
        openai_instrumentor = try_import_instrumentor('openinference.instrumentation.openai', 'OpenAIInstrumentor')
        if openai_instrumentor:
            instrumentors.append(openai_instrumentor)
    
    logger.info(f"🧰 Using instrumentors for framework '{framework}': {[i.__name__ for i in instrumentors]}")
    return instrumentors

# --- Internals ---

class ContextSessionIdProcessor(SpanProcessor):
    def on_start(self, span, parent_context):
        if sid := session_id_ctx.get(): span.set_attribute("session.id", sid)
    def on_end(self, span): pass
    def shutdown(self): pass
    def force_flush(self, timeout_millis=30000): pass


def _patch_uvicorn(project_name):
    try:
        import uvicorn
        if getattr(uvicorn.run, "_is_patched", False): return

        original_run = uvicorn.run
        def patched_run(app, **kwargs):
            logger.info(f"Injecting Tracing Middleware into {type(app)}")
            if hasattr(app, "add_middleware"):
                from starlette.middleware.base import BaseHTTPMiddleware
                app.add_middleware(_JsonRpcSessionMiddleware)

            return original_run(app, **kwargs)
        
        patched_run._is_patched = True
        uvicorn.run = patched_run
    except ImportError:
        pass

# Middleware class definition (simplified)
try:
    from starlette.middleware.base import BaseHTTPMiddleware
    class _JsonRpcSessionMiddleware(BaseHTTPMiddleware):
        async def dispatch(self, request, call_next):
            token = None
            try:
                body = await request.body()
                if body and (data := json.loads(body)) and isinstance(data, dict):
                    if sid := data.get("id"):
                        token = session_id_ctx.set(sid)
                        if span := trace.get_current_span(): span.set_attribute("session.id", sid)
                
                async def receive(): return {"type": "http.request", "body": body}
                request._receive = receive
            except Exception: pass
            
            try: return await call_next(request)
            finally: 
                if token: session_id_ctx.reset(token)
except ImportError:
    _JsonRpcSessionMiddleware = None
